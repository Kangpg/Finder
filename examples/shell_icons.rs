use anyhow::{Result, ensure};
use finder::{native_drag::Apartment, shell_icon};
use std::{
    ffi::OsString,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};
fn fixture_command(program: &str, args: Vec<OsString>) -> Result<Vec<u8>> {
    let output = Command::new(program)
        .args(args)
        .env_remove("LANG")
        .env_remove("LC_ALL")
        .env_remove("LC_CTYPE")
        .output()?;
    ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output.stdout)
}
fn main() -> Result<()> {
    let _ole = Apartment::initialize()?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    let wc = temp.path().join("한글 wc");
    fixture_command("svnadmin", vec!["create".into(), repo.as_os_str().into()])?;
    let url = format!(
        "file:///{}",
        dunce::simplified(&repo)
            .to_str()
            .unwrap()
            .replace('\\', "/")
    );
    fixture_command(
        "svn",
        vec!["checkout".into(), url.into(), wc.as_os_str().into()],
    )?;
    for name in [
        "clean.txt",
        "modified.txt",
        "new.txt",
        "untracked.txt",
        "ignored.tmp",
    ] {
        std::fs::write(wc.join(name), name)?;
    }
    fixture_command(
        "svn",
        vec![
            "add".into(),
            wc.join("clean.txt").into_os_string(),
            wc.join("modified.txt").into_os_string(),
        ],
    )?;
    fixture_command(
        "svn",
        vec![
            "commit".into(),
            wc.as_os_str().into(),
            "-m".into(),
            "temporary local fixture".into(),
        ],
    )?;
    std::fs::write(wc.join("modified.txt"), "changed")?;
    fixture_command(
        "svn",
        vec!["add".into(), wc.join("new.txt").into_os_string()],
    )?;
    fixture_command(
        "svn",
        vec![
            "propset".into(),
            "svn:ignore".into(),
            "*.tmp".into(),
            wc.as_os_str().into(),
        ],
    )?;
    let status = fixture_command(
        "svn",
        vec![
            "status".into(),
            "--xml".into(),
            "--verbose".into(),
            "--no-ignore".into(),
            wc.as_os_str().into(),
        ],
    )?;
    std::fs::create_dir_all("artifacts")?;
    std::fs::write("artifacts/shell-icons-svn-status.xml", status)?;
    let paths: Vec<_> = [
        "clean.txt",
        "modified.txt",
        "new.txt",
        "untracked.txt",
        "ignored.tmp",
    ]
    .iter()
    .map(|s| wc.join(s))
    .chain([wc.clone(), Path::new(r"C:\Windows\explorer.exe").into()])
    .collect();
    let names = [
        "clean",
        "modified",
        "added",
        "unversioned",
        "ignored",
        "folder",
        "explorer.exe",
    ];
    let menus = finder::shell_menu::inspect(&[wc.join("clean.txt")], false)?;
    std::fs::write(
        "artifacts/shell-menu-svn.json",
        serde_json::to_vec_pretty(&menus)?,
    )?;
    let start = Instant::now();
    let mut icons = Vec::new();
    for pass in 0..4 {
        icons = paths
            .iter()
            .map(|p| shell_icon::read(p))
            .collect::<Result<Vec<_>>>()?;
        if pass < 3 {
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    let width = paths.len() * 48;
    let height = 96;
    let mut atlas = vec![0u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let p = (y * width + x) * 4;
            let b = if y < 48 { 35 } else { 240 };
            atlas[p..p + 4].copy_from_slice(&[b, b, b, 255]);
        }
    }
    for (n, icon) in icons.iter().enumerate() {
        for row in 0..2 {
            for y in 0..32 {
                for x in 0..32 {
                    let c = icon.image.pixels[y * 32 + x].to_srgba_unmultiplied();
                    let p = ((row * 48 + y + 8) * width + n * 48 + x + 8) * 4;
                    for channel in 0..3 {
                        atlas[p + channel] = ((c[channel] as u32 * c[3] as u32
                            + atlas[p + channel] as u32 * (255 - c[3] as u32)
                            + 127)
                            / 255) as u8;
                    }
                }
            }
        }
    }
    std::fs::write("artifacts/shell-icons.rgba", &atlas)?;
    let values:Vec<_>=icons.iter().enumerate().map(|(n,icon)|serde_json::json!({"label":names[n],"image_index":icon.image_index,"overlay_index":icon.overlay_index,"pixels_blake3":blake3::hash(bytemuck_bytes(&icon.image.pixels).as_slice()).to_hex().as_str()})).collect();
    println!(
        "{}",
        serde_json::json!({"samples":values,"elapsed_ms_including_cache_wait":start.elapsed().as_secs_f64()*1000.,"atlas_width":width,"atlas_height":height,"atlas":"artifacts/shell-icons.rgba","native_overlays":"opaque indexes; provider decides their meaning and availability"})
    );
    Ok(())
}
fn bytemuck_bytes(pixels: &[eframe::egui::Color32]) -> Vec<u8> {
    pixels.iter().flat_map(|p| p.to_array()).collect()
}
