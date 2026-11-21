use gtk4::gdk_pixbuf::PixbufLoader;
use gtk4::prelude::*;
use gtk4::Image;

pub const SVG_RING: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24"><path fill="#e3e3e3" d="M12,6A6,6 0 1,0 18,12A6,6 0 0,0 12,6M12,9A3,3 0 1,1 9,12A3,3 0 0,1 12,9Z"/></svg>"##;
pub const SVG_BRAIN: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24"><path fill="#c4c7c5" d="M13,3A3,3 0 0,1 16,6C16,6.35 15.94,6.68 15.83,7H16.5A2.5,2.5 0 0,1 19,9.5C19,10.22 18.69,10.87 18.19,11.33C18.69,11.83 19,12.53 19,13.3A2.7,2.7 0 0,1 16.3,16H16V17.5A2.5,2.5 0 0,1 13.5,20H12.5V3H13M11,3H11.5V20H10.5A2.5,2.5 0 0,1 8,17.5V16H7.7A2.7,2.7 0 0,1 5,13.3C5,12.53 5.31,11.83 5.81,11.33C5.31,10.87 5,10.22 5,9.5A2.5,2.5 0 0,1 7.5,7H8.17C8.06,6.68 8,6.35 8,6A3,3 0 0,1 11,3Z"/></svg>"##;
pub const SVG_SEARCH_DOC: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24"><path fill="#c4c7c5" d="M14,2H6A2,2 0 0,0 4,4V20A2,2 0 0,0 6,22H12.5C12.18,21.39 12,20.72 12,20H6V4H13V9H18V12.08C18.71,12.22 19.38,12.5 20,12.88V8L14,2M17.5,14A3.5,3.5 0 0,0 14,17.5A3.5,3.5 0 0,0 17.5,21C18.23,21 18.9,20.77 19.45,20.38L21.54,22.46L22.96,21.04L20.88,18.95C21.27,18.4 21.5,17.73 21.5,17.5A3.5,3.5 0 0,0 17.5,14M17.5,16A1.5,1.5 0 0,1 19,17.5A1.5,1.5 0 0,1 17.5,19A1.5,1.5 0 0,1 16,17.5A1.5,1.5 0 0,1 17.5,16Z"/></svg>"##;
pub const SVG_FULLSCREEN: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24"><path fill="#c4c7c5" d="M5,5H10V7H7V10H5V5M14,5H19V10H17V7H14V5M17,14H19V19H14V17H17V14M10,17V19H5V14H7V17H10Z"/></svg>"##;
pub const SVG_CLOSE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24"><path fill="#c4c7c5" d="M19,6.41L17.59,5L12,10.59L6.41,5L5,6.41L10.59,12L5,17.59L6.41,19L12,13.41L17.59,19L19,17.59L13.41,12L19,6.41Z"/></svg>"##;

pub fn svg_image(svg: &str, size: i32) -> Image {
    let loader = PixbufLoader::new();
    let _ = loader.write(svg.as_bytes());
    let _ = loader.close();
    if let Some(pb) = loader.pixbuf() {
        let img = Image::from_pixbuf(Some(&pb));
        img.set_pixel_size(size);
        img
    } else {
        Image::new()
    }
}

pub fn studio_tile_svg(kind: &str) -> (&'static str, &'static str) {
    match kind {
        "audio" => ("#a8c7fa", "M12,14C13.66,14 15,12.66 15,11V5C15,3.34 13.66,2 12,2C10.34,2 9,3.34 9,5V11C9,12.66 10.34,14 12,14M17,11C17,13.76 14.76,16 12,16C9.24,16 7,13.76 7,11H5C5,14.53 7.61,17.43 11,17.92V21H13V17.92C16.39,17.43 19,14.53 19,11H17Z"),
        "slides" => ("#fdd663", "M19,3H5C3.89,3 3,3.89 3,5V19C3,20.11 3.89,21 5,21H19C20.11,21 21,20.11 21,19V5C21,3.89 20.11,3 19,3M19,19H5V5H19V19M7,10H9V17H7V10M11,7H13V17H11V7M15,13H17V17H15V13Z"),
        "mindmap" => ("#c58af9", "M12,2A3,3 0 0,1 15,5C15,6.28 14.19,7.38 13.06,7.81L14.5,10.5H16A3,3 0 0,1 19,13.5A3,3 0 0,1 16,16.5C14.68,16.5 13.56,15.66 13.16,14.5H10.84C10.44,15.66 9.32,16.5 8,16.5A3,3 0 0,1 5,13.5A3,3 0 0,1 8,10.5H9.5L10.94,7.81C9.81,7.38 9,6.28 9,5A3,3 0 0,1 12,2Z"),
        "report" => ("#81c995", "M14,2H6A2,2 0 0,0 4,4V20A2,2 0 0,0 6,22H18A2,2 0 0,0 20,20V8L14,2M18,20H6V4H13V9H18V20M8,12H16V14H8V12M8,16H13V18H8V16Z"),
        "flashcards" => ("#f6aea9", "M4,6H2V20A2,2 0 0,0 4,22H18V20H4V6M20,2H8A2,2 0 0,0 6,4V16A2,2 0 0,0 8,18H20A2,2 0 0,0 22,16V4A2,2 0 0,0 20,2M20,16H8V4H20V16Z"),
        "quiz" => ("#78d9ec", "M12,2A10,10 0 0,1 22,12A10,10 0 0,1 12,22A10,10 0 0,1 2,12A10,10 0 0,1 12,2M11,16.5L18,9.5L16.59,8.09L11,13.67L7.91,10.59L6.5,12L11,16.5Z"),
        _ => ("#fcad70", "M3,3V21H21V19H5V3H3M9,17H7V10H9V17M13,17H11V7H13V17M17,17H15V13H17V17Z"),
    }
}

pub fn studio_icon_image(kind: &str) -> Image {
    let (color, path) = studio_tile_svg(kind);
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24"><path fill="{color}" d="{path}"/></svg>"##
    );
    svg_image(&svg, 18)
}
