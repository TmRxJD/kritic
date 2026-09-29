//! What this machine has: GPU, free disk, and the external tools kritic can use.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq)]
pub enum Gpu {
    Nvidia { name: String, vram_mb: u64, compute: (u32, u32) },
    AppleSilicon,
    Unknown,
}

impl Gpu {
    pub fn describe(&self) -> String {
        match self {
            Gpu::Nvidia { name, vram_mb, compute } => format!("{name}, {} MB VRAM, compute {}.{}", vram_mb, compute.0, compute.1),
            Gpu::AppleSilicon => "Apple silicon (Metal)".into(),
            Gpu::Unknown => "no NVIDIA or Apple GPU detected".into(),
        }
    }

    /// Local Kev-4B Q8_0 needs about 5 GB of VRAM (measured), plus room for the desktop.
    pub fn fits_local_kev(&self) -> bool {
        match self {
            Gpu::Nvidia { vram_mb, .. } => *vram_mb >= 6000,
            Gpu::AppleSilicon => true,
            Gpu::Unknown => false,
        }
    }
}

pub fn gpu() -> Gpu {
    if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        return Gpu::AppleSilicon;
    }
    let out = Command::new("nvidia-smi").args(["--query-gpu=name,memory.total,compute_cap", "--format=csv,noheader,nounits"]).output();
    let Ok(out) = out else { return Gpu::Unknown };
    if !out.status.success() {
        return Gpu::Unknown;
    }
    parse_nvidia_smi(&String::from_utf8_lossy(&out.stdout)).unwrap_or(Gpu::Unknown)
}

/// First GPU line of `nvidia-smi --query-gpu=name,memory.total,compute_cap`.
pub fn parse_nvidia_smi(text: &str) -> Option<Gpu> {
    let line = text.lines().find(|l| !l.trim().is_empty())?;
    let parts: Vec<&str> = line.split(',').map(str::trim).collect();
    let [name, mem, cap] = parts.as_slice() else { return None };
    let (maj, min) = cap.split_once('.')?;
    Some(Gpu::Nvidia { name: name.to_string(), vram_mb: mem.parse().ok()?, compute: (maj.parse().ok()?, min.parse().ok()?) })
}

/// The laya build for this machine. CUDA PTX runs on its own architecture and newer ones (the
/// driver JIT-compiles it), so pick the newest published build at or below the GPU's compute
/// capability; anything older, or no NVIDIA GPU, uses Vulkan.
pub fn laya_backend(gpu: &Gpu) -> &'static str {
    match gpu {
        Gpu::AppleSilicon => "metal",
        Gpu::Nvidia { compute, .. } => match compute {
            (maj, min) if (*maj, *min) >= (8, 9) => "cuda-sm89",
            (8, min) if *min >= 6 => "cuda-sm86",
            (8, _) => "cuda-sm80",
            _ => "vulkan",
        },
        Gpu::Unknown => "vulkan",
    }
}

/// The laya release asset for this OS and backend.
pub fn laya_asset(backend: &str) -> String {
    if cfg!(windows) {
        format!("laya-windows-x86_64-{backend}.zip")
    } else if cfg!(target_os = "macos") {
        "laya-macos-arm64-metal.tar.gz".into()
    } else {
        format!("laya-linux-x86_64-{backend}.tar.gz")
    }
}

/// Free bytes on the disk holding `path` (or its nearest existing ancestor).
pub fn free_space(path: &Path) -> Option<u64> {
    let mut p = path.to_path_buf();
    while !p.exists() {
        p = p.parent()?.to_path_buf();
    }
    let p = crate::files::dunce_canonical(&p);
    let disks = sysinfo::Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .filter(|d| p.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space())
}

/// `name` on PATH, or in one of `extra` directories.
pub fn tool(name: &str, extra: &[&Path]) -> Option<PathBuf> {
    let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    for d in extra {
        let p = d.join(&exe);
        if p.is_file() {
            return Some(p);
        }
    }
    crate::config::which(name).or_else(|| {
        // npm, npx and friends are .cmd shims on Windows.
        if cfg!(windows) { std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(format!("{name}.cmd"))).find(|p| p.is_file()) } else { None }
    })
}

/// First line of `<tool> --version`, for display.
pub fn version(tool: &Path) -> Option<String> {
    let out = Command::new(tool).arg("--version").output().ok()?;
    let text = String::from_utf8_lossy(if out.stdout.is_empty() { &out.stderr } else { &out.stdout }).to_string();
    text.lines().next().map(|l| l.trim().to_string()).filter(|l| !l.is_empty())
}
