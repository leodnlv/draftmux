use gpui::App;

pub struct AppearanceObserver;
pub fn observe_appearance(_: &mut App) -> AppearanceObserver {
    AppearanceObserver
}
pub fn window_dark(window: &gpui::Window, _: &App) -> bool {
    matches!(
        window.appearance(),
        gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark
    )
}
pub fn system_dark(cx: &App) -> bool {
    matches!(
        cx.window_appearance(),
        gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark
    )
}
pub fn reduce_transparency() -> bool {
    false
}
/// The desktop entry and the Start Menu shortcut run `draftmux app`, so a
/// bare command is never the app's launch.
pub fn launched_as_app() -> bool {
    false
}
pub fn reduce_motion() -> bool {
    false
}

/// Whether one of the app's windows has focus.
pub fn app_active(cx: &mut gpui::App) -> bool {
    cx.windows().into_iter().any(|window| {
        window
            .update(cx, |_, window, _| window.is_window_active())
            .unwrap_or(false)
    })
}

/// Retaining the clipboard keeps X11 selection ownership alive.
struct RichClipboard(arboard::Clipboard);
impl gpui::Global for RichClipboard {}

pub fn write_flavours(flavours: &crate::copy::Flavours, cx: &mut App) -> bool {
    // Linux consumers use HTML rather than RTF, and Slack's own format
    // needs a clipboard type arboard can't write.
    let _ = (&flavours.rtf, &flavours.chromium);
    if !cx.has_global::<RichClipboard>() {
        match arboard::Clipboard::new() {
            Ok(clipboard) => cx.set_global(RichClipboard(clipboard)),
            Err(error) => {
                eprintln!("draftmux: rich clipboard unavailable: {error}");
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(flavours.plain.clone()));
                return false;
            }
        }
    }
    match cx
        .global_mut::<RichClipboard>()
        .0
        .set_html(flavours.html.clone(), Some(flavours.plain.clone()))
    {
        Ok(()) => true,
        Err(error) => {
            eprintln!("draftmux: rich clipboard unavailable: {error}");
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(flavours.plain.clone()));
            false
        }
    }
}

/// The label of a button that reveals a path in the file manager.
pub const REVEAL_LABEL: &str = if cfg!(windows) {
    "Show in Explorer"
} else {
    "Show in File Manager"
};

pub fn window_options(mut options: gpui::WindowOptions) -> gpui::WindowOptions {
    if let Some(titlebar) = options.titlebar.as_mut() {
        titlebar.appears_transparent = false;
        titlebar.traffic_light_position = None;
    }
    options.window_background = gpui::WindowBackgroundAppearance::Opaque;
    options.app_id = Some("fi.zendit.Draftmux".into());
    options
}
pub fn toolbar_items(palette: &crate::theme::Palette) -> Option<gpui::AnyElement> {
    use gpui::{MouseButton, div, prelude::*, px};
    Some(
        div()
            .id("open-settings")
            .mx(px(12.))
            .my(px(10.))
            .px(px(8.))
            .py(px(4.))
            .text_size(px(12.))
            .rounded(px(5.))
            .hover(|s| s.bg(palette.hover))
            .active(|style| style.opacity(0.8))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(|_, _, cx| crate::settings_window::open(cx))
            .child("Settings")
            .into_any_element(),
    )
}
pub fn menu_color(palette: &crate::theme::Palette) -> gpui::Hsla {
    palette.menu
}
/// Keeps pop-up items at their 5px radius inside the padding.
pub const POPUP_RADIUS: f32 = 10.;

pub fn popup_background() -> gpui::WindowBackgroundAppearance {
    gpui::WindowBackgroundAppearance::Opaque
}

/// The compositor draws pop-up shadows the same whatever has the keyboard.
pub fn keep_inactive_appearance(_: &gpui::Window) {}

/// A pop-up resized natively; there is none on Linux, where gpui resizes
/// them.
pub enum NativeWindow {}

pub fn prepare_popup(_: &gpui::Window, _: bool) -> Option<NativeWindow> {
    None
}

pub fn move_popup(
    window: &NativeWindow,
    _: gpui::Bounds<gpui::Pixels>,
    _: gpui::Bounds<gpui::Pixels>,
    _: bool,
) {
    match *window {}
}
#[cfg(target_os = "linux")]
#[path = "platform_linux/frame.rs"]
mod frame;
#[cfg(target_os = "linux")]
pub use frame::frame;

/// Windows draws the title bar and window frame.
#[cfg(windows)]
pub fn frame(
    content: gpui::AnyElement,
    _: &mut gpui::Window,
    cx: &mut App,
    _close: Box<dyn gpui::Action>,
    _title: &'static str,
) -> gpui::AnyElement {
    backed(content, cx)
}

/// Translucent surfaces need an opaque backing on non-blurred windows.
fn backed(content: gpui::AnyElement, cx: &App) -> gpui::AnyElement {
    use gpui::{div, prelude::*};
    div()
        .size_full()
        .bg(crate::theme::palette(cx).pane)
        .child(content)
        .into_any_element()
}

/// Detaches from the console window a Start Menu or Explorer launch of the
/// console program creates, so none stays open beside the app. A console
/// shared with a terminal is kept for the app's output.
#[cfg(windows)]
pub fn release_console() {
    use windows_sys::Win32::System::Console::{FreeConsole, GetConsoleProcessList};
    let mut processes = [0u32; 2];
    // SAFETY: the buffer holds the two entries the call may write.
    let count = unsafe { GetConsoleProcessList(processes.as_mut_ptr(), 2) };
    if count == 1 {
        // SAFETY: a plain call; stderr is redirected to the log afterwards.
        unsafe { FreeConsole() };
    }
}
