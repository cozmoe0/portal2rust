// portal2rust's page loader.
//
// Sets up the global `p2r` object the game (WebAssembly) calls into, then:
//   1. downloads the server's index of the game's files,
//   2. fetches the files the menus need ("boot" phase): from this browser's
//      IndexedDB when it has them from an earlier visit, the rest from the
//      server in a few large parallel "packs",
//   3. loads the player's saved files from IndexedDB,
//   4. starts the game (compiled while 1-3 ran) once Play is clicked.
//
// The game reads files synchronously. Files are kept in memory here; before
// a map loads the game calls `p2r.ensure(map)`, which fetches everything the
// map read the last time anyone played it, and waits behind its loading
// screen. A file nobody listed yet is fetched synchronously (slow, but only
// once: the game reports what each map read, and the server remembers).
'use strict';

(() => {
  // The first warnings and errors, kept for diagnosis (`p2r.errors` in the
  // browser's console) after the console itself has scrolled them away.
  const errors = [];
  // Timestamped game log lines about maps (`p2r.marks`), for load timings.
  const marks = [];
  for (const level of ['log', 'warn', 'error']) {
    const original = console[level].bind(console);
    console[level] = (...args) => {
      const text = args.map(String).join(' ');
      // The game logs every level through console.log.
      if (errors.length < 400 && (level !== 'log' || /ERROR|WARN|panicked/.test(text))) {
        errors.push(`[${level}] ${text.replace(/%c/g, '')}`);
      }
      if (level === 'log' && marks.length < 200 && /loaded|map |quality|render scale/i.test(text)) {
        marks.push([Math.round(performance.now()), text.replace(/%c/g, '').split(' color:')[0]]);
      }
      original(...args);
    };
  }
  // winit asks for the pointer lock outside a click, which browsers refuse;
  // the page asks again on the next click.
  window.addEventListener('unhandledrejection', (e) => {
    if (String(e.reason).includes('Pointer Lock')) e.preventDefault();
  });

  // Time spent in each animation frame's callbacks (the game's whole frame
  // on the CPU, rendering commands included): `p2r.frameStats()`.
  const frameTimes = new Float64Array(600);
  const frameGaps = new Float64Array(600);
  const longFrames = [];
  let frameCount = 0;
  let lastFrameStart = 0;
  const nativeRaf = window.requestAnimationFrame.bind(window);
  window.requestAnimationFrame = (cb) => nativeRaf((ts) => {
    const t0 = performance.now();
    cb(ts);
    const dt = performance.now() - t0;
    frameGaps[frameCount % frameGaps.length] = lastFrameStart ? t0 - lastFrameStart : 0;
    lastFrameStart = t0;
    frameTimes[frameCount++ % frameTimes.length] = dt;
    if (dt > 100 && longFrames.length < 100) longFrames.push([Math.round(t0), Math.round(dt)]);
  });

  const $ = (id) => document.getElementById(id);
  const canvas = $('p2r-canvas');

  const ASSET_DB = 'portal2rust-assets';
  const SAVE_DB = 'portal2rust-saves';
  // Game files kept in memory beyond the menus' and the current maps'.
  const MEMORY_BUDGET = 1024 * 1024 * 1024;
  const PACK_STREAMS = 4;
  const loopback = ['localhost', '127.0.0.1', '[::1]'].includes(location.hostname);

  const state = {
    info: null,
    indexRaw: null,
    versions: new Map(),
    cache: new Map(),
    cacheBytes: 0,
    phases: new Map(),
    phaseOrder: [],
    assetDb: null,
    saveDb: null,
    userFiles: [],
    writeQueue: [],
    writeBytes: 0,
    writeTimer: 0,
    wantLock: false,
    hadLock: false,
    fonts: new Set(),
    pendingFonts: [],
    onDemand: 0,
    started: false,
    lastMap: null,
  };

  let resolveReady, rejectReady;
  const ready = new Promise((res, rej) => { resolveReady = res; rejectReady = rej; });
  let resolvePlay;
  const playClicked = new Promise((res) => { resolvePlay = res; });

  // ---------------------------------------------------------------- UI

  function progress(fraction, text) {
    $('bar').style.width = `${Math.max(0, Math.min(1, fraction)) * 100}%`;
    if (text !== undefined) $('progress-text').textContent = text;
  }

  function fatal(message, title) {
    console.error(message);
    $('error-title').textContent = title || 'Something went wrong';
    $('error-text').textContent = String(message);
    $('error').style.display = 'flex';
    $('menu').classList.add('hidden');
    rejectReady?.(new Error(String(message)));
  }

  const mib = (n) => `${(n / 1048576).toFixed(n < 10485760 ? 1 : 0)} MiB`;

  async function showStorage() {
    try {
      const est = await navigator.storage.estimate();
      $('storage').textContent = `${mib(est.usage || 0)} stored`;
    } catch { /* not available */ }
  }

  function updateHint() {
    const locked = document.pointerLockElement === canvas;
    $('hint').style.display = state.started && state.wantLock && !locked ? 'block' : 'none';
  }

  // ---------------------------------------------------------- IndexedDB

  function openDb(name, store) {
    return new Promise((resolve, reject) => {
      const req = indexedDB.open(name, 1);
      req.onupgradeneeded = () => req.result.createObjectStore(store);
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => reject(req.error);
    });
  }

  function idbGetMany(keys) {
    const found = new Map();
    if (!state.assetDb || keys.length === 0) return Promise.resolve(found);
    return new Promise((resolve) => {
      const tx = state.assetDb.transaction('files', 'readonly');
      const store = tx.objectStore('files');
      for (const key of keys) {
        const req = store.get(key);
        req.onsuccess = () => { if (req.result) found.set(key, req.result); };
      }
      tx.oncomplete = () => resolve(found);
      tx.onerror = tx.onabort = () => resolve(found);
    });
  }

  // Writes downloaded files to IndexedDB in batches, off the hot path.
  function queueWrite(key, version, data) {
    if (!state.assetDb) return;
    state.writeQueue.push([key, version, data]);
    state.writeBytes += data.length;
    if (state.writeBytes > 64 * 1048576) flushWrites();
    else if (!state.writeTimer) state.writeTimer = setTimeout(flushWrites, 400);
  }

  function flushWrites() {
    clearTimeout(state.writeTimer);
    state.writeTimer = 0;
    const batch = state.writeQueue;
    state.writeQueue = [];
    state.writeBytes = 0;
    if (!batch.length || !state.assetDb) return;
    try {
      const tx = state.assetDb.transaction('files', 'readwrite');
      const store = tx.objectStore('files');
      for (const [key, v, d] of batch) store.put({ v, d }, key);
      tx.onerror = () => console.warn('portal2rust: could not cache files', tx.error);
    } catch (e) {
      console.warn('portal2rust: could not cache files', e);
    }
  }

  async function loadUserFiles() {
    state.saveDb = await openDb(SAVE_DB, 'user');
    state.userFiles = await new Promise((resolve) => {
      const out = [];
      const tx = state.saveDb.transaction('user', 'readonly');
      const req = tx.objectStore('user').openCursor();
      req.onsuccess = () => {
        const c = req.result;
        if (!c) return;
        const d = c.value.d;
        out.push([String(c.key), d instanceof Uint8Array ? d : new Uint8Array(d), c.value.m || 0]);
        c.continue();
      };
      tx.oncomplete = () => resolve(out);
      tx.onerror = () => resolve(out);
    });
    navigator.storage?.persist?.().catch(() => {});
  }

  // ------------------------------------------------------- file cache

  function put(key, data, version) {
    const old = state.cache.get(key);
    if (old) state.cacheBytes -= old.length;
    state.cache.set(key, data);
    state.cacheBytes += data.length;
    queueWrite(key, version, data);
  }

  // Drops files no recent phase needs once memory use passes the budget
  // (they stay in IndexedDB, so a later `ensure` brings them back fast).
  function evict() {
    if (state.cacheBytes <= MEMORY_BUDGET) return;
    const keep = new Set();
    const recent = ['boot', ...state.phaseOrder.slice(-2)];
    for (const name of recent) for (const k of state.phases.get(name)?.keys || []) keep.add(k);
    for (const [key, data] of state.cache) {
      if (!keep.has(key)) {
        state.cache.delete(key);
        state.cacheBytes -= data.length;
      }
    }
    for (const name of [...state.phases.keys()]) if (!recent.includes(name)) state.phases.delete(name);
  }

  // Parses a pack stream: [u32 key length][key][u32 data length | ~0][data]...
  function packParser(onFile) {
    const header = new Uint8Array(4);
    const decoder = new TextDecoder();
    let headerFill = 0;
    let stage = 0; // 0 key length, 1 key, 2 data length, 3 data
    let keyBuf = null, keyFill = 0, key = '';
    let data = null, dataFill = 0;
    return (chunk) => {
      let i = 0;
      while (i < chunk.length) {
        if (stage === 0 || stage === 2) {
          const n = Math.min(4 - headerFill, chunk.length - i);
          header.set(chunk.subarray(i, i + n), headerFill);
          headerFill += n;
          i += n;
          if (headerFill < 4) break;
          headerFill = 0;
          const v = (header[0] | (header[1] << 8) | (header[2] << 16) | (header[3] << 24)) >>> 0;
          if (stage === 0) {
            keyBuf = new Uint8Array(v);
            keyFill = 0;
            stage = 1;
            if (v === 0) { key = ''; stage = 2; }
          } else if (v === 0xffffffff) {
            onFile(key, null);
            stage = 0;
          } else {
            data = new Uint8Array(v);
            dataFill = 0;
            stage = 3;
            if (v === 0) { onFile(key, data); stage = 0; }
          }
        } else if (stage === 1) {
          const n = Math.min(keyBuf.length - keyFill, chunk.length - i);
          keyBuf.set(chunk.subarray(i, i + n), keyFill);
          keyFill += n;
          i += n;
          if (keyFill === keyBuf.length) { key = decoder.decode(keyBuf); stage = 2; }
        } else {
          const n = Math.min(data.length - dataFill, chunk.length - i);
          data.set(chunk.subarray(i, i + n), dataFill);
          dataFill += n;
          i += n;
          if (dataFill === data.length) { onFile(key, data); stage = 0; }
        }
      }
    };
  }

  async function fetchPack(entries, onBytes) {
    const versions = new Map(entries.map((e) => [e.key, e.version]));
    const res = await fetch(`/api/pack${loopback ? '' : '?z=1'}`, {
      method: 'POST',
      body: entries.map((e) => e.key).join('\n'),
      cache: 'no-store',
    });
    if (!res.ok || !res.body) throw new Error(`pack request failed: ${res.status}`);
    const parse = packParser((key, data) => { if (data) put(key, data, versions.get(key) || ''); });
    const reader = res.body.getReader();
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      parse(value);
      onBytes(value.length);
    }
  }

  // Fetches files in a few parallel packs of about equal size.
  async function fetchPacks(entries, onBytes) {
    const lanes = Array.from({ length: PACK_STREAMS }, () => ({ size: 0, list: [] }));
    for (const e of [...entries].sort((a, b) => b.size - a.size)) {
      const lane = lanes.reduce((a, b) => (a.size <= b.size ? a : b));
      lane.list.push(e);
      lane.size += e.size;
    }
    await Promise.all(lanes.filter((l) => l.list.length).map((l) => fetchPack(l.list, onBytes)));
  }

  // Everything a phase read last time, into memory: from IndexedDB first,
  // the rest from the server.
  async function prefetch(phase, entry, report) {
    const started = performance.now();
    const res = await fetch(`/api/manifest/${encodeURIComponent(phase)}`, { cache: 'no-store' });
    const text = res.ok ? await res.text() : '';
    entry.next = res.headers.get('x-p2r-next');
    const list = [];
    for (const line of text.split('\n')) {
      const [key, size, version] = line.split('\t');
      if (key) list.push({ key, size: Number(size) || 0, version: version || '' });
    }
    entry.keys = new Set(list.map((e) => e.key));
    const need = list.filter((e) => !state.cache.has(e.key));
    const total = need.reduce((n, e) => n + e.size, 0) || 1;
    let done = 0;
    const tick = () => {
      entry.progress = Math.min(0.999, done / total);
      report?.(entry.progress, done, total);
    };
    const stored = await idbGetMany(need.map((e) => e.key));
    const missing = [];
    let local = 0;
    for (const e of need) {
      const rec = stored.get(e.key);
      if (rec && rec.v === e.version && rec.d) {
        const data = rec.d instanceof Uint8Array ? rec.d : new Uint8Array(rec.d);
        state.cache.set(e.key, data);
        state.cacheBytes += data.length;
        done += e.size;
        local += e.size;
      } else {
        missing.push(e);
      }
    }
    tick();
    if (missing.length) await fetchPacks(missing, (n) => { done += n; tick(); });
    flushWrites();
    evict();
    const secs = ((performance.now() - started) / 1000).toFixed(1);
    console.info(`portal2rust: ${phase}: ${list.length} files (${mib(local)} from this browser, ` +
      `${mib(missing.reduce((n, e) => n + e.size, 0))} from the server) in ${secs}s`);
  }

  // Starts fetching a phase (once); the game waits on `entry.progress`.
  function startPrefetch(phase) {
    let entry = state.phases.get(phase);
    if (entry) return entry;
    entry = { progress: 0, keys: new Set(), next: null };
    state.phases.set(phase, entry);
    state.phaseOrder = [...state.phaseOrder.filter((p) => p !== phase), phase];
    prefetch(phase, entry)
      .catch((e) => console.warn(`portal2rust: prefetch of ${phase} failed; files load on demand`, e))
      .finally(() => {
        entry.progress = 1;
        // The map usually played next comes in behind this one, so the
        // level change finds its files in memory.
        if (entry.next && phase === state.lastMap) {
          setTimeout(() => { if (state.lastMap === phase) startPrefetch(entry.next); }, 15000);
        }
      });
    return entry;
  }

  // ------------------------------------------------------- the p2r API

  const p2r = (window.p2r = {});
  p2r.errors = errors;
  p2r.marks = marks;
  p2r.audioState = () => audioState();
  // For the game's dynamic quality: [median frame interval, 90th percentile
  // interval, median CPU time, frames] over the last 30 frames, in ms.
  p2r.perf = () => {
    const n = Math.min(frameCount, 30);
    const pick = (buf) => {
      const out = new Float64Array(n);
      for (let i = 0; i < n; i++) out[i] = buf[(frameCount - 1 - i + buf.length) % buf.length];
      return out.sort();
    };
    if (n === 0) return new Float64Array([0, 0, 0, 0]);
    const g = pick(frameGaps);
    const t = pick(frameTimes);
    return new Float64Array([g[n >> 1], g[Math.min(n - 1, Math.floor(n * 0.9))], t[n >> 1], n]);
  };

  // `last`: only the most recent frames (default: the whole window of 600).
  p2r.frameStats = (last = 600) => {
    const n = Math.min(frameCount, frameTimes.length, last);
    const recent = (buf) => Array.from({ length: n }, (_, i) => buf[(frameCount - 1 - i + buf.length * 2) % buf.length]).sort((a, b) => a - b);
    const t = recent(frameTimes);
    const g = recent(frameGaps);
    const at = (arr, q) => (arr.length ? arr[Math.min(arr.length - 1, Math.floor(q * arr.length))].toFixed(2) : null);
    return {
      frames: frameCount,
      cpuMedianMs: at(t, 0.5),
      cpuP90Ms: at(t, 0.9),
      cpuMaxMs: at(t, 1),
      frameMedianMs: at(g, 0.5),
      frameP90Ms: at(g, 0.9),
      longFrames,
    };
  };

  p2r.ready = () => ready;

  p2r.takeIndex = () => {
    const raw = state.indexRaw;
    state.indexRaw = null;
    return raw;
  };

  p2r.looseDirs = () => state.info?.loose || [];

  p2r.args = () => {
    const out = [];
    for (const [k, v] of new URLSearchParams(location.search)) {
      const words = v.split(/\s+/).filter(Boolean);
      if (k === 'map') out.push(v);
      else if (k === 'args') out.push(...words);
      else if (k.startsWith('+') || k.startsWith(' ')) out.push(`+${k.trim()}`, ...words);
      else {
        out.push(k.startsWith('-') ? k : `--${k}`);
        if (v !== '') out.push(v);
      }
    }
    return out;
  };

  p2r.cacheGet = (key) => state.cache.get(key);

  // A file nobody listed for this phase yet: fetched synchronously.
  p2r.fetchSync = (key) => {
    const started = performance.now();
    const xhr = new XMLHttpRequest();
    xhr.open('GET', `/api/file/${encodeURIComponent(key)}`, false);
    xhr.overrideMimeType('text/plain; charset=x-user-defined');
    try {
      xhr.send();
    } catch (e) {
      console.warn(`portal2rust: ${key}: ${e}`);
      return undefined;
    }
    if (xhr.status !== 200) return undefined;
    const text = xhr.responseText;
    const data = new Uint8Array(text.length);
    for (let i = 0; i < text.length; i++) data[i] = text.charCodeAt(i) & 0xff;
    put(key, data, xhr.getResponseHeader('x-p2r-version') || state.versions.get(key) || '');
    state.onDemand++;
    console.debug(`portal2rust: fetched ${key} on demand (${data.length} bytes, ${(performance.now() - started).toFixed(0)} ms)`);
    return data;
  };

  p2r.ensure = (phase) => {
    phase = phase.toLowerCase();
    if (phase !== state.lastMap) {
      // A level change: the server learns which map follows which.
      if (state.lastMap) fetch(`/api/next/${encodeURIComponent(state.lastMap)}`, { method: 'POST', body: phase }).catch(() => {});
      state.lastMap = phase;
      const known = state.phases.get(phase);
      if (known && known.progress >= 1 && known.next) {
        setTimeout(() => { if (state.lastMap === phase) startPrefetch(known.next); }, 15000);
      }
    }
    return startPrefetch(phase).progress;
  };

  p2r.report = (phase, files) => {
    const body = String(files);
    fetch(`/api/learn/${encodeURIComponent(phase.toLowerCase())}`, {
      method: 'POST',
      body,
      keepalive: body.length < 60000,
    }).catch(() => {});
  };

  p2r.userFiles = () => {
    const files = state.userFiles;
    state.userFiles = [];
    return files;
  };

  p2r.userPut = (path, data, modified) => {
    const copy = data.slice(); // `data` is a view into the game's memory
    if (!state.saveDb) return;
    const tx = state.saveDb.transaction('user', 'readwrite');
    tx.objectStore('user').put({ d: copy, m: modified }, path);
    tx.onerror = () => console.error(`portal2rust: could not save ${path}`, tx.error);
  };

  p2r.userDelete = (path) => {
    if (!state.saveDb) return;
    state.saveDb.transaction('user', 'readwrite').objectStore('user').delete(path);
  };

  p2r.status = (text) => {
    const el = $('status');
    el.textContent = text;
    el.style.display = text ? 'block' : 'none';
  };

  p2r.fatal = (message) => fatal(message, 'The game stopped');

  // Render scale: the canvas (through its stage) takes a smaller CSS size
  // and is scaled back up to the page, so the game renders fewer pixels.
  // Mouse positions stay right: they are read in the canvas's own space.
  const stage = $('stage');
  p2r.viewportPixels = () => window.innerWidth * window.innerHeight * (window.devicePixelRatio || 1) ** 2;
  p2r.setRenderScale = (s) => {
    // The game sizes its canvas in device pixels; the stage is in CSS ones.
    const css = s / (window.devicePixelRatio || 1);
    if (css >= 0.999) {
      stage.style.cssText = '';
      return;
    }
    stage.style.cssText = `inset:auto;left:0;top:0;width:${css * 100}vw;height:${css * 100}vh;` +
      `transform:scale(${1 / css});transform-origin:0 0`;
  };

  // Console commands for the game, from the browser's developer console:
  // `p2r.console('r_fx_preset 2')`.
  const commands = [];
  p2r.console = (text) => { commands.push(String(text)); };
  p2r.takeCommands = () => (commands.length ? commands.splice(0).join('\n') : undefined);

  // ------------------------------------------------------ pointer lock

  function lockPointer() {
    try {
      const p = canvas.requestPointerLock({ unadjustedMovement: true });
      if (p && p.catch) p.catch(() => { try { canvas.requestPointerLock(); } catch { /* refused */ } });
    } catch {
      try { canvas.requestPointerLock(); } catch { /* refused */ }
    }
  }

  p2r.wantPointerLock = (on) => {
    state.wantLock = !!on;
    if (!on && document.pointerLockElement === canvas) document.exitPointerLock();
    updateHint();
  };

  canvas.addEventListener('mousedown', () => {
    canvas.focus();
    if (state.wantLock && document.pointerLockElement !== canvas) lockPointer();
  });
  canvas.addEventListener('contextmenu', (e) => e.preventDefault());

  // The browser releases the pointer on Esc without telling the page about
  // the key; the game then gets an Escape of its own (its pause menu).
  document.addEventListener('pointerlockchange', () => {
    const locked = document.pointerLockElement === canvas;
    if (!locked && state.hadLock && state.wantLock) {
      setTimeout(() => {
        if (!state.wantLock || document.pointerLockElement === canvas) return;
        for (const type of ['keydown', 'keyup']) {
          canvas.dispatchEvent(new KeyboardEvent(type, { key: 'Escape', code: 'Escape', bubbles: true, cancelable: true }));
        }
      }, 60);
    }
    state.hadLock = locked;
    updateHint();
  });

  // ------------------------------------------------------------- WebGPU

  // wgpu reads `device.limits` and `device.features` afresh on every query
  // and Bevy queries the limits for every draw; the browser builds a new
  // object behind each read. Both are fixed for a device's life, so the
  // first read is kept as a plain object (and a plain Set).
  if (window.GPUDevice) {
    const limits = Object.getOwnPropertyDescriptor(GPUDevice.prototype, 'limits');
    const features = Object.getOwnPropertyDescriptor(GPUDevice.prototype, 'features');
    const kept = new WeakMap();
    const keep = (device) => {
      let k = kept.get(device);
      if (!k) kept.set(device, (k = {}));
      return k;
    };
    if (limits?.get) {
      Object.defineProperty(GPUDevice.prototype, 'limits', {
        configurable: true,
        get() {
          const k = keep(this);
          if (!k.limits) {
            const live = limits.get.call(this);
            const plain = {};
            for (const name in live) if (typeof live[name] === 'number') plain[name] = live[name];
            k.limits = Object.freeze(plain);
          }
          return k.limits;
        },
      });
    }
    if (features?.get) {
      Object.defineProperty(GPUDevice.prototype, 'features', {
        configurable: true,
        get() {
          const k = keep(this);
          return (k.features ??= new Set(features.get.call(this)));
        },
      });
    }
  }

  // ------------------------------------------------------------- audio

  // Browsers start audio only after a click or key press; resume whatever
  // the game created once there has been one.
  const contexts = [];
  if (window.AudioContext) {
    const Base = window.AudioContext;
    window.AudioContext = class extends Base {
      constructor(...args) {
        super(...args);
        contexts.push(this);
        if (navigator.userActivation?.hasBeenActive) this.resume().catch(() => {});
      }
    };
  }
  const resumeAudio = () => contexts.forEach((c) => c.state === 'suspended' && c.resume().catch(() => {}));
  const audioState = () => contexts.map((c) => `${c.state} ${c.sampleRate} Hz`);
  window.addEventListener('pointerdown', resumeAudio, true);
  window.addEventListener('keydown', resumeAudio, true);

  // The game mixes on the main thread a little ahead of time into a ring
  // buffer of shared memory, and an audio worklet plays it on the browser's
  // audio thread. A frame that runs long then costs at most a moment of
  // silence (faded, and counted so that the game mixes further ahead),
  // never the stutter of audio buffers scheduled late.
  const WORKLET = `
class P2RMix extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const o = options.processorOptions;
    this.data = new Float32Array(o.data);
    this.ctl = new Int32Array(o.ctl);
    this.mask = o.cap - 1;
    this.playing = false;
  }
  process(inputs, outputs) {
    const out = outputs[0];
    const L = out[0], R = out[1] || out[0], n = L.length;
    const ctl = this.ctl, d = this.data, mask = this.mask;
    const r = Atomics.load(ctl, 0), avail = (Atomics.load(ctl, 1) - r) | 0;
    const m = avail < n ? Math.max(avail, 0) : n;
    const fadeIn = !this.playing && m > 0 ? Math.min(m, 64) : 0;
    for (let i = 0; i < m; i++) {
      const j = ((r + i) & mask) * 2;
      const g = i < fadeIn ? i / fadeIn : 1;
      L[i] = d[j] * g;
      R[i] = d[j + 1] * g;
    }
    if (m < n) {
      const fade = Math.min(m, 32);
      for (let i = 0; i < fade; i++) {
        const g = (fade - i) / (fade + 1);
        L[m - fade + i] *= g;
        R[m - fade + i] *= g;
      }
      for (let i = m; i < n; i++) { L[i] = 0; R[i] = 0; }
      if (this.playing) Atomics.add(ctl, 2, 1);
      this.playing = false;
    } else {
      this.playing = true;
    }
    Atomics.store(ctl, 0, (r + m) | 0);
    return true;
  }
}
registerProcessor('p2r-mix', P2RMix);
`;
  let ring = null;
  // Starts the worklet; the output rate, or undefined without shared memory
  // (the game then uses its Web Audio fallback).
  p2r.audioOpen = () => {
    if (!window.crossOriginIsolated || typeof SharedArrayBuffer !== 'function' || !window.AudioWorkletNode) return undefined;
    try {
      const ctx = new AudioContext({ latencyHint: 'interactive' });
      const cap = 1 << 16;
      const data = new Float32Array(new SharedArrayBuffer(cap * 2 * 4));
      const ctl = new Int32Array(new SharedArrayBuffer(16));
      ring = { ctx, data, ctl, cap };
      const url = URL.createObjectURL(new Blob([WORKLET], { type: 'text/javascript' }));
      ctx.audioWorklet.addModule(url).then(() => {
        const node = new AudioWorkletNode(ctx, 'p2r-mix', {
          numberOfInputs: 0,
          numberOfOutputs: 1,
          outputChannelCount: [2],
          processorOptions: { data: data.buffer, ctl: ctl.buffer, cap },
        });
        node.connect(ctx.destination);
        ring.node = node;
      }).catch((e) => console.warn('portal2rust: audio worklet failed', e));
      return ctx.sampleRate;
    } catch (e) {
      console.warn('portal2rust: no audio worklet', e);
      ring = null;
      return undefined;
    }
  };
  const ringStatus = new Int32Array(2);
  // [frames queued, underruns so far]
  p2r.audioStatus = () => {
    if (ring) {
      ringStatus[0] = (Atomics.load(ring.ctl, 1) - Atomics.load(ring.ctl, 0)) | 0;
      ringStatus[1] = Atomics.load(ring.ctl, 2);
    }
    return ringStatus;
  };
  // Appends interleaved stereo frames (a view into the game's memory).
  p2r.audioPush = (samples) => {
    if (!ring) return;
    const { data, ctl, cap } = ring;
    const w = Atomics.load(ctl, 1);
    const free = cap - ((w - Atomics.load(ctl, 0)) | 0);
    const frames = Math.min(samples.length >> 1, free);
    const start = w & (cap - 1);
    const first = Math.min(frames, cap - start);
    data.set(samples.subarray(0, first * 2), start * 2);
    if (frames > first) data.set(samples.subarray(first * 2, frames * 2), 0);
    Atomics.store(ctl, 1, (w + frames) | 0);
  };

  // ------------------------------------------------------------- fonts

  const fontCanvas = typeof OffscreenCanvas === 'function' ? new OffscreenCanvas(256, 256) : document.createElement('canvas');
  const fctx = fontCanvas.getContext('2d', { willReadFrequently: true });
  const fonts = [];
  const fontIds = new Map();
  let currentFont = '';

  function setFont(css) {
    if (currentFont !== css) {
      fctx.font = css;
      currentFont = css;
    }
  }

  const cssFont = (face, px, weight, italic) =>
    `${italic ? 'italic ' : ''}${Math.max(1, Math.min(1000, weight || 400))} ${px}px "${face}", Arial, Helvetica, sans-serif`;

  // A face under each of its names (family, full and PostScript names), with
  // its own weight and style so that the browser only emboldens or slants
  // it where GDI would.
  p2r.fontRegister = (names, weight, italic, data) => {
    const bytes = data.slice(); // `data` is a view into the game's memory
    let added = false;
    for (const family of String(names).split('\n').filter(Boolean)) {
      const key = `${family}|${weight}|${italic}`;
      if (state.fonts.has(key)) { added = true; continue; }
      try {
        const face = new FontFace(family, bytes, { weight: String(weight || 400), style: italic ? 'italic' : 'normal' });
        document.fonts.add(face);
        state.pendingFonts.push(face.load().catch((e) => console.warn(`portal2rust: font ${family}`, e)));
        state.fonts.add(key);
        added = true;
      } catch (e) {
        console.warn(`portal2rust: font ${family}`, e);
      }
    }
    return added;
  };

  p2r.fontsReady = () => Promise.all(state.pendingFonts);

  // A font sized like GDI sizes a cell: ascent + descent = tall.
  p2r.fontCreate = (face, tall, weight, italic) => {
    const key = `${face}|${tall}|${weight}|${italic}`;
    let id = fontIds.get(key);
    if (id === undefined) {
      setFont(cssFont(face, 100, weight, italic));
      let m = fctx.measureText('Hg');
      const px = (tall * 100) / Math.max(1, m.fontBoundingBoxAscent + m.fontBoundingBoxDescent);
      const css = cssFont(face, px.toFixed(3), weight, italic);
      setFont(css);
      m = fctx.measureText('Hg');
      const ascent = Math.round(m.fontBoundingBoxAscent);
      let maxWidth = 0;
      for (let c = 32; c < 127; c++) maxWidth = Math.max(maxWidth, Math.ceil(fctx.measureText(String.fromCharCode(c)).width));
      id = fonts.length;
      fonts.push({ css, ascent, descent: Math.max(0, tall - ascent), maxWidth });
      fontIds.set(key, id);
    }
    const f = fonts[id];
    return new Float64Array([id, f.ascent, f.descent, f.maxWidth]);
  };

  p2r.glyphMetrics = (id, ch) => {
    setFont(fonts[id].css);
    const m = fctx.measureText(ch);
    return new Float64Array([m.width, m.actualBoundingBoxLeft, m.actualBoundingBoxRight]);
  };

  p2r.glyphDraw = (id, ch, w, h, x, y) => {
    if (fontCanvas.width < w || fontCanvas.height < h) {
      fontCanvas.width = Math.max(fontCanvas.width, w);
      fontCanvas.height = Math.max(fontCanvas.height, h);
      currentFont = '';
    }
    fctx.clearRect(0, 0, w, h);
    setFont(fonts[id].css);
    fctx.fillStyle = '#fff';
    fctx.textBaseline = 'alphabetic';
    fctx.textAlign = 'left';
    fctx.fillText(ch, x, y);
    const px = fctx.getImageData(0, 0, w, h).data;
    const out = new Uint8Array(w * h);
    for (let i = 0; i < out.length; i++) out[i] = px[i * 4 + 3];
    return out;
  };

  // -------------------------------------------------------------- boot

  async function checkGpu() {
    if (!window.isSecureContext) {
      throw Object.assign(new Error(
        `This page was opened over plain HTTP from another machine, and browsers only allow WebGPU on secure pages.\n\n` +
        `Open https://${location.host}/ instead (p2web serves HTTPS unless started with --http), ` +
        `or open the page on the server itself at http://localhost:${location.port}/.`), { title: 'WebGPU needs a secure page' });
    }
    if (!navigator.gpu) {
      throw Object.assign(new Error(
        'This browser has no WebGPU. Use a current Chrome or Edge, Firefox 141 or newer on Windows, or Safari 26 or newer.'),
        { title: 'WebGPU is not available' });
    }
    const adapter = await navigator.gpu.requestAdapter({ powerPreference: 'high-performance' });
    if (!adapter) throw Object.assign(new Error('The browser found no usable GPU for WebGPU.'), { title: 'No GPU' });
  }

  // The engine: downloaded and compiled while the files load.
  async function loadEngine(onBytes) {
    const res = await fetch('pkg/portal2_bg.wasm', { cache: 'no-cache' });
    if (!res.ok) throw new Error('The server has no browser build yet: run `p2web build` on it.');
    const total = Number(res.headers.get('x-p2r-size')) || 0;
    let loaded = 0;
    const counted = new Response(new ReadableStream({
      async start(controller) {
        const reader = res.body.getReader();
        for (;;) {
          const { done, value } = await reader.read();
          if (done) break;
          loaded += value.length;
          onBytes(loaded, total);
          controller.enqueue(value);
        }
        controller.close();
      },
    }), { headers: { 'Content-Type': 'application/wasm' } });
    const module = await import('./pkg/portal2.js');
    const exports = await module.default({ module_or_path: counted });
    // The game's memory, for `p2r.memoryMiB()` and the frame statistics.
    p2r.memoryMiB = () => Math.round(exports.memory.buffer.byteLength / 1048576);
  }

  async function boot() {
    $('host').textContent = location.host;
    showStorage();
    await checkGpu();

    let engine = { loaded: 0, total: 0 };
    let files = { done: 0, total: 0 };
    let shown = true;
    const show = () => {
      if (!shown) return;
      const t = engine.total + files.total;
      const d = engine.loaded + files.done;
      progress(t ? d / t : 0, `Downloading: engine ${mib(engine.loaded)}${engine.total ? ` of ${mib(engine.total)}` : ''}` +
        (files.total ? `, menu files ${mib(files.done)} of ${mib(files.total)}` : ''));
    };
    const engineDone = loadEngine((loaded, total) => { engine = { loaded, total }; show(); }).catch((e) => fatal(e.message || e));

    state.info = await (await fetch('api/info', { cache: 'no-store' })).json();
    const indexRes = await fetch('api/index', { cache: 'no-store' });
    state.indexRaw = new Uint8Array(await indexRes.arrayBuffer());
    for (const line of new TextDecoder().decode(state.indexRaw).split('\n')) {
      const tab = line.indexOf('\t');
      if (tab > 0) state.versions.set(line.slice(0, tab), line.slice(line.lastIndexOf('\t') + 1));
    }
    try {
      state.assetDb = await openDb(ASSET_DB, 'files');
    } catch (e) {
      console.warn('portal2rust: no IndexedDB; files are not kept between visits', e);
    }
    await loadUserFiles();

    const bootEntry = { progress: 0, keys: new Set() };
    state.phases.set('boot', bootEntry);
    await prefetch('boot', bootEntry, (_, done, total) => { files = { done, total }; show(); });
    bootEntry.progress = 1;
    // Compiled, and waiting in `p2r.ready` for Play.
    await engineDone;
    shown = false;
    progress(1, `Ready: ${state.info.files.toLocaleString()} game files on the server.`);

    const play = $('play');
    play.disabled = false;
    play.textContent = 'Play';
    await playClicked;
    $('menu').classList.add('hidden');
    p2r.playedAt = Math.round(performance.now());
    p2r.status('Starting…');
    state.started = true;
    canvas.focus();
    resolveReady();
    p2r.status('');
    setTimeout(() => console.info(`portal2rust: ${state.onDemand} files fetched on demand so far`), 30000);
  }

  $('play').addEventListener('click', async () => {
    if ($('fullscreen').checked && document.documentElement.requestFullscreen) {
      try {
        await document.documentElement.requestFullscreen({ navigationUI: 'hide' });
        // In fullscreen, Chrome can hand Esc to the game instead of leaving.
        await navigator.keyboard?.lock?.(['Escape']);
      } catch { /* stays windowed */ }
    }
    resolvePlay();
  });

  $('clear').addEventListener('click', async () => {
    if (state.started) return;
    if (!confirm('Delete the game files this browser downloaded? Saved games are kept.')) return;
    state.assetDb?.close();
    state.assetDb = null;
    indexedDB.deleteDatabase(ASSET_DB);
    location.reload();
  });

  window.addEventListener('pagehide', flushWrites);

  // A reloaded page leaves its GPU device behind until the garbage
  // collector gets to it, with all of its video memory; destroying it as
  // the page goes frees that at once.
  const devices = [];
  if (window.GPUAdapter) {
    const requestDevice = GPUAdapter.prototype.requestDevice;
    GPUAdapter.prototype.requestDevice = function (...args) {
      return requestDevice.apply(this, args).then((device) => {
        devices.push(device);
        return device;
      });
    };
  }
  window.addEventListener('pagehide', (e) => {
    if (e.persisted) return; // kept for the back/forward cache
    for (const device of devices) {
      try { device.destroy(); } catch { /* already gone */ }
    }
  });

  boot().catch((e) => fatal(e.message || e, e.title));
})();
