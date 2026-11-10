// Catppuccin Frappé, matching ~/config/home/ghostty.conf and
// ~/config/home/rex/catppuccin-frappe.conf. UI text uses terminal ANSI blue.
pub(super) const ACCENT: [u8; 3] = [0x8c, 0xaa, 0xee];
pub(super) const FOREGROUND: [u8; 3] = [0xc6, 0xd0, 0xf5];
pub(super) const GRID: [u8; 3] = [0x94, 0x9c, 0xbb];
pub(super) fn color(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}
