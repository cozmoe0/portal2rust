//! Running in the browser: starting once the page's loader is ready, the
//! canvas window, and the bits of the page the game drives (the pointer
//! lock and the error overlay).

use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PresentMode, PrimaryWindow, WindowResolution};
use wasm_bindgen::prelude::*;

/// Rust's own allocator for the browser grows the page's memory exactly as
/// far as each allocation needs, 64 KiB at a time, and every `memory.grow`
/// costs the browser a fair amount: loading a map grew it ten thousand times
/// and stalled for seconds. This is the same allocator (dlmalloc) taking
/// memory in steps of a quarter of the heap (16 to 256 MiB).
mod heap {
    use std::alloc::{GlobalAlloc, Layout};
    use std::cell::UnsafeCell;

    use core::arch::wasm32;

    const PAGE: usize = 65536;

    struct Chunked;

    // SAFETY: memory from `memory.grow` is fresh, zeroed and never handed out
    // twice; nothing is ever returned to the browser.
    unsafe impl dlmalloc::Allocator for Chunked {
        fn alloc(&self, size: usize) -> (*mut u8, usize, u32) {
            let current = wasm32::memory_size(0) * PAGE;
            let step = size.max((current / 4).clamp(16 << 20, 256 << 20));
            for bytes in [step, size] {
                let pages = bytes.div_ceil(PAGE);
                let prev = wasm32::memory_grow(0, pages);
                if prev != usize::MAX {
                    return ((prev * PAGE) as *mut u8, pages * PAGE, 0);
                }
            }
            (std::ptr::null_mut(), 0, 0)
        }
        fn remap(&self, _ptr: *mut u8, _old: usize, _new: usize, _can_move: bool) -> *mut u8 {
            std::ptr::null_mut()
        }
        fn free_part(&self, _ptr: *mut u8, _old: usize, _new: usize) -> bool {
            false
        }
        fn free(&self, _ptr: *mut u8, _size: usize) -> bool {
            false
        }
        fn can_release_part(&self, _flags: u32) -> bool {
            false
        }
        fn allocates_zeros(&self) -> bool {
            true
        }
        fn page_size(&self) -> usize {
            PAGE
        }
    }

    struct Heap(UnsafeCell<dlmalloc::Dlmalloc<Chunked>>);

    // SAFETY: the page's WebAssembly runs on one thread.
    unsafe impl Sync for Heap {}

    unsafe impl GlobalAlloc for Heap {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            unsafe { (*self.0.get()).malloc(layout.size(), layout.align()) }
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            unsafe { (*self.0.get()).calloc(layout.size(), layout.align()) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            unsafe { (*self.0.get()).free(ptr, layout.size(), layout.align()) }
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            unsafe { (*self.0.get()).realloc(ptr, layout.size(), layout.align(), new_size) }
        }
    }

    #[global_allocator]
    static HEAP: Heap = Heap(UnsafeCell::new(dlmalloc::Dlmalloc::new_with_allocator(Chunked)));
}

#[wasm_bindgen]
extern "C" {
    /// Shows a fatal error over the page.
    #[wasm_bindgen(js_namespace = p2r)]
    fn fatal(message: &str);
}

/// Waits for the page's loader (the game's index, the menus' files, the
/// player's saves and the custom fonts), then runs the app.
pub fn start(run: fn()) {
    install_panic_overlay();
    wasm_bindgen_futures::spawn_local(async move {
        let fs = portal2_core::web::boot().await;
        portal2_ui::vgui::font::preload_web_fonts(&fs).await;
        run();
        // The app's own panic handler is in place now; keep the overlay.
        install_panic_overlay();
    });
}

/// Panics also show on the page (and still go to the console).
fn install_panic_overlay() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        previous(info);
        fatal(&info.to_string());
    }));
}

/// The page's canvas, filling the page; the game draws at the page's CSS
/// size (not the display's device pixels), as cheap as the desktop's
/// default window.
pub fn window() -> Window {
    Window {
        title: "Portal 2 (Rust)".into(),
        canvas: Some("#p2r-canvas".into()),
        fit_canvas_to_parent: true,
        // Keys like Tab, F-keys and the space bar belong to the game.
        prevent_default_event_handling: true,
        resolution: WindowResolution::default().with_scale_factor_override(1.0),
        present_mode: PresentMode::AutoVsync,
        ..default()
    }
}

pub struct WebPlugin;

impl Plugin for WebPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Last, follow_cursor_grab);
    }
}

/// Browsers only lock the pointer from a click: the page does that on the
/// next click on the canvas while the game wants the mouse captured, and
/// turns the browser's own release (Esc) into the game's Escape key.
fn follow_cursor_grab(cursor: Query<&CursorOptions, (With<PrimaryWindow>, Changed<CursorOptions>)>) {
    if let Ok(c) = cursor.single() {
        portal2_core::web::want_pointer_lock(c.grab_mode != CursorGrabMode::None);
    }
}
