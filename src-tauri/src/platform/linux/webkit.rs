//! WebKitGTK process-wide prep. Must run before GTK starts.

/// WebKitGTK's DMA-BUF renderer needs a DRM device. A VNC or llvmpipe session
/// has none, and the panel paints black (or not at all). The env has to be
/// set before GTK starts; a reader who already chose a value is left alone.
pub fn prepare() {
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_some() {
        return;
    }
    let has_drm = std::path::Path::new("/dev/dri/card0").exists()
        || std::path::Path::new("/dev/dri/renderD128").exists();
    if has_drm {
        return;
    }
    // SAFETY: called from `main` before any other threads exist.
    // DMA-BUF is the path that needs a DRM device. Compositing stays on:
    // CSS `backdrop-filter` is the Linux frost, and it needs a compositor.
    unsafe {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
}
