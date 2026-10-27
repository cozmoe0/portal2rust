//! Decodes every Bink file of the local Portal 2 installation. The tests pass
//! without doing anything when no installation is found; set `PORTAL2_DIR`
//! to the folder containing `portal2.exe` to point them at one.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use bink::{AudioDecoder, Bink, Frame};

fn media_dir() -> Option<PathBuf> {
    let mut roots: Vec<PathBuf> = std::env::var_os("PORTAL2_DIR").map(PathBuf::from).into_iter().collect();
    roots.push(PathBuf::from(r"C:\Program Files (x86)\Steam\steamapps\common\Portal 2"));
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(home).join(".local/share/Steam/steamapps/common/Portal 2"));
    }
    roots.into_iter().map(|r| r.join("portal2").join("media")).find(|m| m.is_dir())
}

fn bik_files(media: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(media)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("bik")))
        .collect();
    files.sort();
    files
}

fn fnv(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

fn plane_hashes(f: &Frame) -> [u64; 3] {
    [fnv(&f.y.data), fnv(&f.u.data), fnv(&f.v.data)]
}

/// FNV-1a hashes of the Y, Cb and Cr planes (block padding included) of
/// frames as decoded by the reference decoder, `binkw32.dll`.
const REFERENCE: &[(&str, u32, [u64; 3])] = &[
    ("menu_act01", 0, [0xcf83eabd01a16c94, 0xa7e88445bc98f7dc, 0x947302310da4a99f]),
    ("menu_act01", 287, [0x0995432fe45815eb, 0x868bec551fec550f, 0x6d07565d09ca2adb]),
    ("valve", 150, [0x68c0b18b2a9295f9, 0xbb079d39c1713819, 0x4f6c3acbb7c784c1]),
    ("sp_a5_credits", 450, [0xfa34807ec0704c07, 0xe6dc5324e864809c, 0xca8371aa18895891]),
    ("coop_bts_radar_1", 100, [0x1792f730b93e0f12, 0x488700934d247376, 0x76da72abbcc66e8e]),
    ("insert_disk", 47, [0x2a7d3cda71c76380, 0xbfe37269d077a7b3, 0x45f6ea70831e2617]),
    ("attract02", 1000, [0x81a912c6e7086149, 0x456e4fbf289766e1, 0xd02502ba18d80364]),
];

/// Decodes all frames and audio packets of one file.
fn check_file(path: &Path) -> Result<(), String> {
    let name = path.file_stem().unwrap().to_string_lossy().to_lowercase();
    let mut video = Bink::open(std::fs::read(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let mut tracks: Vec<AudioDecoder> =
        video.audio_tracks().iter().map(AudioDecoder::new).collect::<Result<_, _>>().map_err(|e| e.to_string())?;
    let mut pcm = Vec::new();
    for n in 0..video.frame_count() {
        for (t, decoder) in tracks.iter_mut().enumerate() {
            let Some(packet) = video.audio_packet(n, t).map_err(|e| e.to_string())? else { continue };
            pcm.clear();
            let samples = decoder.decode(packet, &mut pcm).map_err(|e| format!("frame {n} track {t}: {e}"))?;
            let declared = u32::from_le_bytes(packet[..4].try_into().unwrap()) as usize / 2;
            if samples != declared {
                return Err(format!("frame {n} track {t}: {samples} samples, {declared} declared"));
            }
        }
        let frame = video.next_frame().map_err(|e| e.to_string())?;
        if frame.index() != n {
            return Err(format!("frame {n} reports index {}", frame.index()));
        }
        for (_, _, want) in REFERENCE.iter().filter(|(f, i, _)| *f == name && *i == n) {
            if plane_hashes(frame) != *want {
                return Err(format!("frame {n} differs from the reference decoder"));
            }
        }
    }
    if video.next_frame().is_ok() {
        return Err("decoded past the last frame".into());
    }
    Ok(())
}

#[test]
fn decodes_every_installed_video() {
    let Some(media) = media_dir() else {
        eprintln!("Portal 2 installation not found, skipping");
        return;
    };
    let files = bik_files(&media);
    assert!(!files.is_empty(), "no .bik files in {}", media.display());
    let next = AtomicUsize::new(0);
    let failures = Mutex::new(Vec::new());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                while let Some(path) = files.get(next.fetch_add(1, Ordering::Relaxed)) {
                    if let Err(e) = check_file(path) {
                        failures.lock().unwrap().push(format!("{}: {e}", path.display()));
                    }
                }
            });
        }
    });
    let failures = failures.into_inner().unwrap();
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn seeking_matches_sequential_decoding() {
    let Some(media) = media_dir() else {
        eprintln!("Portal 2 installation not found, skipping");
        return;
    };
    let data = std::fs::read(media.join("valve.bik")).unwrap();
    let mut video = Bink::open(data.clone()).unwrap();
    let mut hashes = Vec::new();
    while video.position() < video.frame_count() {
        hashes.push(plane_hashes(video.next_frame().unwrap()));
    }
    assert!((1..video.frame_count()).any(|f| video.is_keyframe(f)), "expected keyframes after the first frame");
    let mut video = Bink::open(data).unwrap();
    for target in [200, 37, 312, 0, 150, 151] {
        video.seek(target).unwrap();
        let frame = video.next_frame().unwrap();
        assert_eq!(frame.index(), target);
        assert_eq!(plane_hashes(frame), hashes[target as usize], "frame {target}");
    }
    assert!(video.seek(video.frame_count()).is_err());
}
