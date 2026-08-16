// Build script: generates the Aura icon (violet aura orb) as a .ico and embeds it as a resource.
use std::env;
use std::fs;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/aura.ico");
    let out_dir = env::var("OUT_DIR").unwrap();

    // Bevorzugt das gepflegte Mehrgroessen-Icon aus assets/ (siehe
    // tools/make_icon.py). Fehlt es, wird ein einfaches 32x32 erzeugt, damit
    // der Build ohne Python funktioniert.
    let repo_ico = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets").join("aura.ico");
    let ico_path = if repo_ico.is_file() {
        repo_ico
    } else {
        let p = Path::new(&out_dir).join("aura.ico");
        write_icon(&p);
        p
    };

    let rc_path = Path::new(&out_dir).join("aura.rc");
    let ico_fwd = ico_path.to_string_lossy().replace('\\', "/");
    fs::write(&rc_path, format!("1 ICON \"{}\"", ico_fwd)).unwrap();
    embed_resource::compile(&rc_path, embed_resource::NONE);

    copy_webview2_loader(Path::new(&out_dir));
}

/// The exe imports WebView2Loader.dll, so it has to sit next to the binary.
/// webview2-com-sys stages it in its own OUT_DIR; copy it into the profile dir.
fn copy_webview2_loader(out_dir: &Path) {
    // OUT_DIR = <target>/<profile>/build/<pkg>-<hash>/out
    let build_dir = match out_dir.parent().and_then(|p| p.parent()) {
        Some(p) => p.to_path_buf(),
        None => return,
    };
    let profile_dir = match build_dir.parent() {
        Some(p) => p.to_path_buf(),
        None => return,
    };
    let arch = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86_64") => "x64",
        Ok("x86") => "x86",
        Ok("aarch64") => "arm64",
        _ => return,
    };
    let entries = match fs::read_dir(&build_dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("webview2-com-sys-") {
            continue;
        }
        let src = entry.path().join("out").join(arch).join("WebView2Loader.dll");
        if src.is_file() {
            let dest = profile_dir.join("WebView2Loader.dll");
            let _ = fs::copy(&src, &dest);
            // Deps dir too, so `cargo run` and test binaries find it as well.
            let deps = profile_dir.join("deps");
            if deps.is_dir() {
                let _ = fs::copy(&src, deps.join("WebView2Loader.dll"));
            }
            return;
        }
    }
}

/// Writes a 32x32 32bpp ICO with a soft violet radial-gradient orb.
fn write_icon(path: &Path) {
    const S: usize = 32;
    let mut px = vec![0u8; S * S * 4]; // BGRA, bottom-up rows written later
    let cx = (S as f32 - 1.0) / 2.0;
    let cy = cx;
    for y in 0..S {
        for x in 0..S {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let d = (dx * dx + dy * dy).sqrt() / (S as f32 / 2.0);
            // soft orb: full alpha center -> transparent edge
            let alpha = (1.0 - d).clamp(0.0, 1.0);
            let alpha = alpha.powf(0.75);
            // radial gradient: deep violet center -> light lavender edge
            let t = d.clamp(0.0, 1.0);
            let (r, g, b) = (
                lerp(0x4A as f32, 0xC9 as f32, t),
                lerp(0x3D as f32, 0xBE as f32, t),
                lerp(0xC8 as f32, 0xF2 as f32, t),
            );
            let a = (alpha * 255.0).round() as u8;
            let i = (y * S + x) * 4;
            px[i] = b as u8;
            px[i + 1] = g as u8;
            px[i + 2] = r as u8;
            px[i + 3] = a;
        }
    }
    // ICO: ICONDIR + ICONDIRENTRY + BITMAPINFOHEADER(h=2*S) + XOR pixels(bottom-up) + AND mask
    let mut data = Vec::new();
    data.extend_from_slice(&[0, 0, 1, 0]); // reserved, type=icon
    data.extend_from_slice(&1u16.to_le_bytes()); // count
    let img_size = 40 + px.len() + (S * 32 / 8); // header + xor + AND mask rows (4 bytes each)
    data.push(S as u8); // width
    data.push(S as u8); // height
    data.push(0); // colors
    data.push(0); // reserved
    data.extend_from_slice(&1u16.to_le_bytes()); // planes
    data.extend_from_slice(&32u16.to_le_bytes()); // bitcount
    data.extend_from_slice(&(img_size as u32).to_le_bytes());
    data.extend_from_slice(&22u32.to_le_bytes()); // offset
    // BITMAPINFOHEADER
    data.extend_from_slice(&40u32.to_le_bytes());
    data.extend_from_slice(&(S as i32).to_le_bytes());
    data.extend_from_slice(&((S as i32) * 2).to_le_bytes()); // height*2 (xor+and)
    data.extend_from_slice(&1u16.to_le_bytes()); // planes
    data.extend_from_slice(&32u16.to_le_bytes()); // bpp
    data.extend_from_slice(&0u32.to_le_bytes()); // compression BI_RGB
    data.extend_from_slice(&0u32.to_le_bytes()); // size image
    data.extend_from_slice(&0i32.to_le_bytes());
    data.extend_from_slice(&0i32.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    // XOR bitmap bottom-up
    for y in (0..S).rev() {
        data.extend_from_slice(&px[y * S * 4..(y + 1) * S * 4]);
    }
    // AND mask: all zeros (fully opaque), 4 bytes per row
    data.extend(std::iter::repeat(0u8).take(S * 4));
    fs::write(path, data).unwrap();
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
