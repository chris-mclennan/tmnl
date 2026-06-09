use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Clone)]
struct FontSpec {
    path: PathBuf,
    collection_index: u32,
}

impl FontSpec {
    fn file(p: impl Into<PathBuf>) -> Self {
        Self {
            path: p.into(),
            collection_index: 0,
        }
    }
    fn ttc(p: impl Into<PathBuf>, idx: u32) -> Self {
        Self {
            path: p.into(),
            collection_index: idx,
        }
    }
}

/// Build the per-style font fallback chains. Tries Nerd Fonts first so
/// mnml's UI icons (file-tree chevrons, devicons, git glyphs in the
/// private-use range) render; appends system mono fonts as backstop.
fn discover_font_chains() -> [Vec<FontSpec>; 4] {
    let mut regular: Vec<FontSpec> = Vec::new();
    let mut bold: Vec<FontSpec> = Vec::new();
    let mut italic: Vec<FontSpec> = Vec::new();
    let mut bold_italic: Vec<FontSpec> = Vec::new();

    // Per-OS font-search-path conventions. macOS uses `~/Library/Fonts`
    // and `/Library/Fonts`; Linux follows `XDG_DATA_HOME` / freedesktop
    // (`~/.local/share/fonts`, `/usr/share/fonts`, `/usr/local/share/fonts`,
    // `~/.fonts` for the legacy path); Windows uses `%LOCALAPPDATA%\
    // Microsoft\Windows\Fonts` plus `C:\Windows\Fonts`. We probe all of
    // them — the first family that matches wins, so adding paths is
    // harmless on platforms where they don't exist.
    let mut font_dirs: Vec<PathBuf> = Vec::new();
    let home = std::env::var("HOME").ok();
    #[cfg(target_os = "macos")]
    {
        if let Some(h) = &home {
            font_dirs.push(PathBuf::from(h).join("Library/Fonts"));
        }
        font_dirs.push(PathBuf::from("/Library/Fonts"));
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
            font_dirs.push(PathBuf::from(xdg).join("fonts"));
        } else if let Some(h) = &home {
            font_dirs.push(PathBuf::from(h).join(".local/share/fonts"));
        }
        if let Some(h) = &home {
            font_dirs.push(PathBuf::from(h).join(".fonts"));
        }
        font_dirs.push(PathBuf::from("/usr/local/share/fonts"));
        font_dirs.push(PathBuf::from("/usr/share/fonts"));
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            font_dirs.push(PathBuf::from(local).join("Microsoft/Windows/Fonts"));
        }
        if let Ok(win) = std::env::var("WINDIR") {
            font_dirs.push(PathBuf::from(win).join("Fonts"));
        }
    }

    // Nerd Font families to try, in priority order. "Mono" variants enforce
    // strict single-cell width — required for our grid model. The `-mnml`
    // suffix is the family patched with Claude/Codex glyphs at U+F8B0 /
    // U+F8B1 (`scripts/patch_nerd_font.py` in the mnml repo); preferring
    // it means native tmnl ships those custom glyphs, falling back to the
    // upstream JetBrainsMono Nerd Font when the patched variant isn't
    // installed.
    let nerd_families: &[(&str, &str, &str, &str)] = &[
        (
            "JetBrainsMonoNerdFontMono-Regular-mnml.ttf",
            "JetBrainsMonoNerdFontMono-Bold.ttf",
            "JetBrainsMonoNerdFontMono-Italic.ttf",
            "JetBrainsMonoNerdFontMono-BoldItalic.ttf",
        ),
        (
            "JetBrainsMonoNerdFontMono-Regular.ttf",
            "JetBrainsMonoNerdFontMono-Bold.ttf",
            "JetBrainsMonoNerdFontMono-Italic.ttf",
            "JetBrainsMonoNerdFontMono-BoldItalic.ttf",
        ),
        (
            "FiraCodeNerdFontMono-Regular.ttf",
            "FiraCodeNerdFontMono-Bold.ttf",
            "FiraCodeNerdFontMono-Retina.ttf",
            "FiraCodeNerdFontMono-Bold.ttf",
        ),
        (
            "HackNerdFontMono-Regular.ttf",
            "HackNerdFontMono-Bold.ttf",
            "HackNerdFontMono-Italic.ttf",
            "HackNerdFontMono-BoldItalic.ttf",
        ),
        (
            "SymbolsNerdFontMono-Regular.ttf",
            "SymbolsNerdFontMono-Regular.ttf",
            "SymbolsNerdFontMono-Regular.ttf",
            "SymbolsNerdFontMono-Regular.ttf",
        ),
        // Non-Mono (proportional) Nerd Font variants — last-resort
        // fallback for users who installed `JetBrainsMonoNerdFont`
        // (the default cask name) instead of the strict-monospace
        // `JetBrainsMonoNerdFontMono`. Powerline arrows + symbols
        // may render slightly wider than one cell, but missing
        // powerline glyphs read worse than mis-aligned ones.
        // 2026-06-09 user report — saw `)` for `` after enabling
        // the themed prompt.
        (
            "JetBrainsMonoNerdFont-Regular.ttf",
            "JetBrainsMonoNerdFont-Bold.ttf",
            "JetBrainsMonoNerdFont-Italic.ttf",
            "JetBrainsMonoNerdFont-BoldItalic.ttf",
        ),
        (
            "FiraCodeNerdFont-Regular.ttf",
            "FiraCodeNerdFont-Bold.ttf",
            "FiraCodeNerdFont-Retina.ttf",
            "FiraCodeNerdFont-Bold.ttf",
        ),
        (
            "HackNerdFont-Regular.ttf",
            "HackNerdFont-Bold.ttf",
            "HackNerdFont-Italic.ttf",
            "HackNerdFont-BoldItalic.ttf",
        ),
    ];
    for dir in &font_dirs {
        for (r, b, i, bi) in nerd_families {
            let rp = dir.join(r);
            if rp.exists() {
                regular.push(FontSpec::file(&rp));
                let bp = dir.join(b);
                if bp.exists() {
                    bold.push(FontSpec::file(&bp));
                }
                let ip = dir.join(i);
                if ip.exists() {
                    italic.push(FontSpec::file(&ip));
                }
                let bip = dir.join(bi);
                if bip.exists() {
                    bold_italic.push(FontSpec::file(&bip));
                }
                // Stop at the first family that matches in this dir.
                break;
            }
        }
    }

    // System-font backstop — keeps plain ASCII rendering even if no
    // Nerd Font is installed. Tries the common monospaced ttf locations
    // on each platform; `FontSpec::file` is a path-existence-checked
    // load so misses are silent and harmless.
    #[cfg(target_os = "macos")]
    {
        regular.push(FontSpec::file("/System/Library/Fonts/SFNSMono.ttf"));
        regular.push(FontSpec::ttc("/System/Library/Fonts/Menlo.ttc", 0));
        regular.push(FontSpec::file("/System/Library/Fonts/Monaco.ttf"));
        bold.push(FontSpec::ttc("/System/Library/Fonts/Menlo.ttc", 1));
        italic.push(FontSpec::file("/System/Library/Fonts/SFNSMonoItalic.ttf"));
        italic.push(FontSpec::ttc("/System/Library/Fonts/Menlo.ttc", 2));
        bold_italic.push(FontSpec::ttc("/System/Library/Fonts/Menlo.ttc", 3));
    }
    #[cfg(target_os = "linux")]
    {
        // DejaVu Sans Mono ships with most distros (Ubuntu, Fedora,
        // Arch). Liberation Mono is the Red Hat / RHEL fallback.
        // Noto Mono is GNOME's default and lands on a lot of modern
        // installs. Paths cover the common locations even though
        // fontconfig usually finds them anyway — direct-path loads
        // sidestep the need to depend on fontconfig at all.
        regular.push(FontSpec::file(
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        ));
        regular.push(FontSpec::file("/usr/share/fonts/dejavu/DejaVuSansMono.ttf"));
        regular.push(FontSpec::file(
            "/usr/share/fonts/liberation/LiberationMono-Regular.ttf",
        ));
        regular.push(FontSpec::file(
            "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
        ));
        regular.push(FontSpec::file(
            "/usr/share/fonts/noto/NotoSansMono-Regular.ttf",
        ));
        bold.push(FontSpec::file(
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf",
        ));
        bold.push(FontSpec::file(
            "/usr/share/fonts/liberation/LiberationMono-Bold.ttf",
        ));
        italic.push(FontSpec::file(
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Oblique.ttf",
        ));
        italic.push(FontSpec::file(
            "/usr/share/fonts/liberation/LiberationMono-Italic.ttf",
        ));
        bold_italic.push(FontSpec::file(
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-BoldOblique.ttf",
        ));
        bold_italic.push(FontSpec::file(
            "/usr/share/fonts/liberation/LiberationMono-BoldItalic.ttf",
        ));
    }
    #[cfg(target_os = "windows")]
    {
        // Consolas ships with every Windows since Vista and is the
        // default monospaced font for Windows Terminal / VS Code on
        // Windows. Cascadia Mono is bundled with newer installs.
        regular.push(FontSpec::file("C:/Windows/Fonts/consola.ttf"));
        regular.push(FontSpec::file("C:/Windows/Fonts/CascadiaMono.ttf"));
        regular.push(FontSpec::file("C:/Windows/Fonts/lucon.ttf"));
        bold.push(FontSpec::file("C:/Windows/Fonts/consolab.ttf"));
        italic.push(FontSpec::file("C:/Windows/Fonts/consolai.ttf"));
        bold_italic.push(FontSpec::file("C:/Windows/Fonts/consolaz.ttf"));
    }

    [regular, bold, italic, bold_italic]
}

pub const STYLE_REGULAR: u8 = 0;
#[allow(dead_code)]
pub const STYLE_BOLD: u8 = 1;
#[allow(dead_code)]
pub const STYLE_ITALIC: u8 = 2;
#[allow(dead_code)]
pub const STYLE_BOLD_ITALIC: u8 = 3;

const ATLAS_W: u32 = 1024;
const ATLAS_H: u32 = 1024;
const PAD: u32 = 1;

#[derive(Clone, Copy, Debug, Default)]
pub struct AtlasGlyph {
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
    pub size: [f32; 2],
    pub offset: [f32; 2],
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphKey {
    ch: u32,
    style: u8,
}

pub struct Atlas {
    #[allow(dead_code)]
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
    glyphs: HashMap<GlyphKey, AtlasGlyph>,
    pub cell_w: f32,
    pub cell_h: f32,
    pub ascent: f32,

    fonts: [Vec<fontdue::Font>; 4],
    px_size: f32,
    pen_x: u32,
    pen_y: u32,
    row_h: u32,
    full: bool,
}

impl Atlas {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, px_size: f32) -> Result<Self, String> {
        let chains = discover_font_chains();
        let mut fonts: [Vec<fontdue::Font>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
        for (style_ix, chain) in chains.iter().enumerate() {
            for spec in chain {
                let Ok(bytes) = std::fs::read(&spec.path) else {
                    continue;
                };
                let settings = fontdue::FontSettings {
                    scale: px_size,
                    collection_index: spec.collection_index,
                    ..Default::default()
                };
                if let Ok(f) = fontdue::Font::from_bytes(bytes, settings) {
                    fonts[style_ix].push(f);
                }
            }
        }
        if fonts[STYLE_REGULAR as usize].is_empty() {
            return Err("no regular font loaded".into());
        }
        if let Some(first) = chains[STYLE_REGULAR as usize].first() {
            log::info!("atlas: primary regular font {}", first.path.display());
        }

        let primary = &fonts[STYLE_REGULAR as usize][0];
        let line = primary
            .horizontal_line_metrics(px_size)
            .ok_or_else(|| "primary font has no horizontal line metrics".to_string())?;
        let cell_h = (line.ascent - line.descent + line.line_gap).ceil();
        let ascent = line.ascent.ceil();

        let (m_metrics, _) = primary.rasterize('M', px_size);
        let cell_w = m_metrics.advance_width.ceil().max(1.0);

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph-atlas"),
            size: wgpu::Extent3d {
                width: ATLAS_W,
                height: ATLAS_H,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // RGBA8 (not R8) so color glyphs (sbix / COLR / CPAL) can land in
            // the same atlas. Monochrome glyphs encode as `(255, 255, 255,
            // grayscale)` — the fragment shader still reads the alpha channel
            // as coverage and uses the per-cell `fg` to tint.
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let zeros = vec![0u8; (ATLAS_W * ATLAS_H * 4) as usize];
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &zeros,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(ATLAS_W * 4),
                rows_per_image: Some(ATLAS_H),
            },
            wgpu::Extent3d {
                width: ATLAS_W,
                height: ATLAS_H,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("glyph-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Ok(Self {
            texture,
            view,
            sampler,
            glyphs: HashMap::new(),
            cell_w,
            cell_h,
            ascent,
            fonts,
            px_size,
            pen_x: PAD,
            pen_y: PAD,
            row_h: 0,
            full: false,
        })
    }

    /// Return the cached glyph for `(ch, style)`, rasterizing on miss.
    /// Style 1..=3 fall back to style 0 (regular) when their chain lacks `ch`.
    pub fn glyph(&mut self, ch: char, style: u8, queue: &wgpu::Queue) -> AtlasGlyph {
        let key = GlyphKey {
            ch: ch as u32,
            style,
        };
        if let Some(&g) = self.glyphs.get(&key) {
            return g;
        }
        if self.full {
            return AtlasGlyph::default();
        }

        let mut chosen: Option<(fontdue::Metrics, Vec<u8>)> = None;
        if let Some(chain) = self.fonts.get(style as usize) {
            for f in chain {
                if f.lookup_glyph_index(ch) != 0 {
                    chosen = Some(f.rasterize(ch, self.px_size));
                    break;
                }
            }
        }
        if chosen.is_none() && style != STYLE_REGULAR {
            for f in &self.fonts[STYLE_REGULAR as usize] {
                if f.lookup_glyph_index(ch) != 0 {
                    chosen = Some(f.rasterize(ch, self.px_size));
                    break;
                }
            }
        }
        let (m, bm) = match chosen {
            Some(x) => x,
            None => {
                self.glyphs.insert(key, AtlasGlyph::default());
                return AtlasGlyph::default();
            }
        };

        let w = m.width as u32;
        let h = m.height as u32;
        if w == 0 || h == 0 {
            let g = AtlasGlyph::default();
            self.glyphs.insert(key, g);
            return g;
        }

        if self.pen_x + w + PAD > ATLAS_W {
            self.pen_x = PAD;
            self.pen_y += self.row_h + PAD;
            self.row_h = 0;
        }
        if self.pen_y + h + PAD > ATLAS_H {
            log::warn!(
                "glyph atlas full at U+{:04X} style {}; subsequent misses → blank",
                key.ch,
                style
            );
            self.full = true;
            return AtlasGlyph::default();
        }

        // Expand fontdue's 8-bit grayscale into RGBA: white tinted by alpha.
        // Color rasterizers (sbix / COLR — task #2b) will fill this region
        // with real RGBA instead.
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for px in &bm {
            rgba.push(0xff);
            rgba.push(0xff);
            rgba.push(0xff);
            rgba.push(*px);
        }
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: self.pen_x,
                    y: self.pen_y,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &rgba,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );

        let offset_y = self.ascent - h as f32 - m.ymin as f32;
        let g = AtlasGlyph {
            uv_min: [
                self.pen_x as f32 / ATLAS_W as f32,
                self.pen_y as f32 / ATLAS_H as f32,
            ],
            uv_max: [
                (self.pen_x + w) as f32 / ATLAS_W as f32,
                (self.pen_y + h) as f32 / ATLAS_H as f32,
            ],
            size: [w as f32, h as f32],
            offset: [m.xmin as f32, offset_y],
        };
        self.glyphs.insert(key, g);
        self.pen_x += w + PAD;
        self.row_h = self.row_h.max(h);
        g
    }
}

pub fn style_from_attrs(attrs: u32) -> u8 {
    const ATTR_BOLD: u32 = 1 << 0;
    const ATTR_ITALIC: u32 = 1 << 2;
    let bold = (attrs & ATTR_BOLD) != 0;
    let italic = (attrs & ATTR_ITALIC) != 0;
    (bold as u8) | ((italic as u8) << 1)
}
