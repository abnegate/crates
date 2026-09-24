use serde::{Deserialize, Serialize};

use crate::hardware::{GpuType, ModelRecommendation, RecommendedModels};

#[cfg(any(target_os = "macos", test))]
const BYTES_PER_GIGABYTE: f64 = 1024.0 * 1024.0 * 1024.0;
#[cfg(any(target_os = "linux", test))]
const MEGABYTES_PER_GIGABYTE: f64 = 1024.0;

/// What a machine can run locally.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MachineProfile {
    pub name: String,
    pub gpu_vram_gb: f64,
    pub system_ram_gb: f64,
    pub unified_memory: bool,
    pub gpu_type: GpuType,
    pub recommended_models: RecommendedModels,
}

impl MachineProfile {
    /// This machine's profile, or a CPU-only one when it cannot be detected.
    ///
    /// Detection runs `sysctl` or `nvidia-smi` and reads `/proc`, so it runs
    /// on the blocking pool rather than stalling the runtime's worker.
    pub async fn detect() -> Self {
        tokio::task::spawn_blocking(Self::detect_blocking)
            .await
            .unwrap_or_else(|_| Self::cpu_only_fallback())
    }

    /// [`Self::detect`] for a caller outside an async runtime. It blocks the
    /// calling thread for as long as the system tools take.
    pub fn detect_blocking() -> Self {
        Self::detect_inner().unwrap_or_else(Self::cpu_only_fallback)
    }

    /// Look up a built-in hardware preset by slug.
    pub fn from_preset(preset: &str) -> Option<Self> {
        match preset {
            "m4-max-64" => Some(Self::m4_max_64()),
            "m4-max-36" => Some(Self::apple_silicon_preset(
                "Apple M4 Max 36GB",
                36.0,
                "M4 Max",
                40,
                Self::mid_apple_models(),
            )),
            "m4-pro-48" => Some(Self::apple_silicon_preset(
                "Apple M4 Pro 48GB",
                48.0,
                "M4 Pro",
                20,
                Self::mid_apple_models(),
            )),
            "m4-pro-24" => Some(Self::apple_silicon_preset(
                "Apple M4 Pro 24GB",
                24.0,
                "M4 Pro",
                20,
                Self::small_apple_models(),
            )),
            "m3-max-96" => Some(Self::apple_silicon_preset(
                "Apple M3 Max 96GB",
                96.0,
                "M3 Max",
                40,
                Self::large_apple_models(),
            )),
            "m3-max-36" => Some(Self::apple_silicon_preset(
                "Apple M3 Max 36GB",
                36.0,
                "M3 Max",
                40,
                Self::mid_apple_models(),
            )),
            "m2-ultra-192" => Some(Self::apple_silicon_preset(
                "Apple M2 Ultra 192GB",
                192.0,
                "M2 Ultra",
                76,
                Self::ultra_apple_models(),
            )),
            "m1-pro-32" => Some(Self::m1_pro_32()),
            "m1-pro-16" => Some(Self::apple_silicon_preset(
                "Apple M1 Pro 16GB",
                16.0,
                "M1 Pro",
                16,
                Self::tiny_apple_models(),
            )),
            "5900x-3080ti" => Some(Self::nvidia_5900x_3080ti()),
            "5900x-3090" => Some(Self::nvidia_desktop_preset(
                "AMD 5900X + RTX 3090",
                24.0,
                64.0,
                "RTX 3090",
                10496,
                Self::nvidia_24gb_models(),
            )),
            "5900x-4090" => Some(Self::nvidia_desktop_preset(
                "AMD/Intel + RTX 4090",
                24.0,
                64.0,
                "RTX 4090",
                16384,
                Self::nvidia_24gb_models(),
            )),
            "13900k-4090" => Some(Self::nvidia_desktop_preset(
                "Intel 13900K + RTX 4090",
                24.0,
                64.0,
                "RTX 4090",
                16384,
                Self::nvidia_24gb_models(),
            )),
            "laptop-4060" => Some(Self {
                name: "Laptop RTX 4060 8GB".into(),
                gpu_vram_gb: 8.0,
                system_ram_gb: 16.0,
                unified_memory: false,
                gpu_type: GpuType::NvidiaLaptop {
                    model: "RTX 4060 Laptop".into(),
                    cuda_cores: 3072,
                },
                recommended_models: Self::nvidia_8gb_laptop_models(),
            }),
            "cpu-only" => Some(Self::cpu_only_fallback()),
            _ => None,
        }
    }

    /// Return the list of all available preset slugs with descriptions.
    pub fn available_presets() -> Vec<(&'static str, &'static str)> {
        vec![
            ("m4-max-64", "Apple M4 Max 64GB -- best local quality"),
            ("m4-max-36", "Apple M4 Max 36GB"),
            ("m4-pro-48", "Apple M4 Pro 48GB"),
            ("m4-pro-24", "Apple M4 Pro 24GB"),
            ("m3-max-96", "Apple M3 Max 96GB"),
            ("m3-max-36", "Apple M3 Max 36GB"),
            ("m2-ultra-192", "Apple M2 Ultra 192GB -- runs everything"),
            ("m1-pro-32", "Apple M1 Pro 32GB"),
            ("m1-pro-16", "Apple M1 Pro 16GB"),
            ("5900x-3080ti", "AMD 5900X + RTX 3080 Ti (12GB VRAM)"),
            ("5900x-3090", "AMD 5900X + RTX 3090 (24GB VRAM)"),
            ("5900x-4090", "AMD/Intel + RTX 4090 (24GB VRAM)"),
            ("13900k-4090", "Intel 13900K + RTX 4090 (24GB VRAM)"),
            ("laptop-4060", "Laptop with RTX 4060 (8GB VRAM)"),
            ("cpu-only", "CPU only -- minimal local models"),
        ]
    }

    /// Maximum VRAM available for concurrent model loading.
    /// For discrete GPUs only VRAM counts. For unified memory the
    /// full pool is available but we reserve 20% for the OS.
    pub fn maximum_concurrent_vram(&self) -> f64 {
        if self.unified_memory {
            self.gpu_vram_gb * 0.80
        } else {
            self.gpu_vram_gb
        }
    }

    /// Effective VRAM available for a single model.
    ///
    /// On unified memory `gpu_vram_gb` already holds the whole shared pool, and
    /// on a discrete GPU it is the card's VRAM with system RAM excluded, so the
    /// answer is `gpu_vram_gb` either way.
    pub fn effective_vram(&self) -> f64 {
        self.gpu_vram_gb
    }

    fn detect_inner() -> Option<Self> {
        #[cfg(target_os = "macos")]
        {
            Self::detect_macos()
        }
        #[cfg(target_os = "linux")]
        {
            Self::detect_linux()
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            None
        }
    }

    #[cfg(target_os = "macos")]
    fn detect_macos() -> Option<Self> {
        let brand = std::process::Command::new("sysctl")
            .args(["-n", "machdep.cpu.brand_string"])
            .output()
            .ok()?;
        let brand = String::from_utf8_lossy(&brand.stdout).trim().to_string();

        let memory = std::process::Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()?;
        let memory_bytes: u64 = String::from_utf8_lossy(&memory.stdout)
            .trim()
            .parse()
            .ok()?;
        let memory_gigabytes = Self::memory_gigabytes(memory_bytes);

        if brand.contains("Apple") {
            let chip = if brand.contains("M4 Max") {
                "M4 Max"
            } else if brand.contains("M4 Pro") {
                "M4 Pro"
            } else if brand.contains("M4") {
                "M4"
            } else if brand.contains("M3 Ultra") {
                "M3 Ultra"
            } else if brand.contains("M3 Max") {
                "M3 Max"
            } else if brand.contains("M3 Pro") {
                "M3 Pro"
            } else if brand.contains("M3") {
                "M3"
            } else if brand.contains("M2 Ultra") {
                "M2 Ultra"
            } else if brand.contains("M2 Max") {
                "M2 Max"
            } else if brand.contains("M2 Pro") {
                "M2 Pro"
            } else if brand.contains("M2") {
                "M2"
            } else if brand.contains("M1 Ultra") {
                "M1 Ultra"
            } else if brand.contains("M1 Max") {
                "M1 Max"
            } else if brand.contains("M1 Pro") {
                "M1 Pro"
            } else if brand.contains("M1") {
                "M1"
            } else {
                "Apple Silicon"
            };

            let gpu_cores = Self::estimate_gpu_cores(chip);
            let models = if memory_gigabytes >= 128.0 {
                Self::ultra_apple_models()
            } else if memory_gigabytes >= 48.0 {
                Self::large_apple_models()
            } else if memory_gigabytes >= 32.0 {
                Self::mid_apple_models()
            } else if memory_gigabytes >= 24.0 {
                Self::small_apple_models()
            } else {
                Self::tiny_apple_models()
            };

            return Some(Self {
                name: format!("Apple {} {}GB", chip, memory_gigabytes as u64),
                gpu_vram_gb: memory_gigabytes,
                system_ram_gb: memory_gigabytes,
                unified_memory: true,
                gpu_type: GpuType::AppleSilicon {
                    chip: chip.into(),
                    gpu_cores,
                },
                recommended_models: models,
            });
        }

        None
    }

    #[cfg(target_os = "linux")]
    fn detect_linux() -> Option<Self> {
        let system_ram_gb = Self::linux_system_ram_gb()?;

        let Some((gpu, vram_gb)) = Self::linux_gpu() else {
            return Some(Self {
                name: format!("Linux CPU-only {}GB RAM", system_ram_gb as u64),
                gpu_vram_gb: 0.0,
                system_ram_gb,
                unified_memory: false,
                gpu_type: GpuType::CpuOnly,
                recommended_models: Self::cpu_only_models(),
            });
        };

        let models = if vram_gb >= 24.0 {
            Self::nvidia_24gb_models()
        } else if vram_gb >= 12.0 {
            Self::nvidia_12gb_models()
        } else {
            Self::nvidia_8gb_laptop_models()
        };

        Some(Self {
            name: format!("Linux + {} {}GB", gpu, vram_gb as u64),
            gpu_vram_gb: vram_gb,
            system_ram_gb,
            unified_memory: false,
            gpu_type: GpuType::NvidiaDesktop {
                model: gpu,
                cuda_cores: 0,
            },
            recommended_models: models,
        })
    }

    /// Installed memory in whole gigabytes, as the machine is sold: 36 GB
    /// reads as 36, not rounded to a multiple of anything.
    #[cfg(any(target_os = "macos", test))]
    fn memory_gigabytes(bytes: u64) -> f64 {
        (bytes as f64 / BYTES_PER_GIGABYTE).round()
    }

    #[cfg(target_os = "linux")]
    fn linux_system_ram_gb() -> Option<f64> {
        let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kilobytes: u64 = meminfo
            .lines()
            .find(|line| line.starts_with("MemTotal:"))?
            .split_whitespace()
            .nth(1)?
            .parse()
            .ok()?;
        Some((kilobytes as f64 / (1024.0 * 1024.0)).round())
    }

    /// The first NVIDIA GPU `nvidia-smi` reports, and its VRAM in gigabytes.
    #[cfg(target_os = "linux")]
    fn linux_gpu() -> Option<(String, f64)> {
        let output = std::process::Command::new("nvidia-smi")
            .args([
                "--query-gpu=name,memory.total",
                "--format=csv,noheader,nounits",
            ])
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        Self::first_gpu(&String::from_utf8_lossy(&output.stdout))
    }

    /// The first line of `nvidia-smi --query-gpu=name,memory.total
    /// --format=csv,noheader,nounits`: one line per GPU, and only the first
    /// describes the card a model is loaded on by default.
    #[cfg(any(target_os = "linux", test))]
    fn first_gpu(reported: &str) -> Option<(String, f64)> {
        let mut fields = reported.lines().next()?.split(',').map(str::trim);
        let name = fields.next().filter(|name| !name.is_empty())?.to_string();
        let megabytes: f64 = fields.next()?.parse().ok()?;

        Some((name, (megabytes / MEGABYTES_PER_GIGABYTE).round()))
    }

    #[cfg(target_os = "macos")]
    fn estimate_gpu_cores(chip: &str) -> u32 {
        match chip {
            "M4 Max" => 40,
            "M4 Pro" => 20,
            "M4" => 10,
            "M3 Ultra" => 76,
            "M3 Max" => 40,
            "M3 Pro" => 18,
            "M3" => 10,
            "M2 Ultra" => 76,
            "M2 Max" => 38,
            "M2 Pro" => 19,
            "M2" => 10,
            "M1 Ultra" => 64,
            "M1 Max" => 32,
            "M1 Pro" => 16,
            "M1" => 8,
            _ => 8,
        }
    }

    fn m4_max_64() -> Self {
        Self {
            name: "Apple M4 Max 64GB".into(),
            gpu_vram_gb: 64.0,
            system_ram_gb: 64.0,
            unified_memory: true,
            gpu_type: GpuType::AppleSilicon {
                chip: "M4 Max".into(),
                gpu_cores: 40,
            },
            recommended_models: RecommendedModels {
                llm: ModelRecommendation {
                    model_name: "qwen2.5:72b-instruct-q6_K".into(),
                    quantization: Some("Q6_K".into()),
                    vram_needed_gb: 55.0,
                    quality_score: 0.92,
                    estimated_speed: "~8 tok/s".into(),
                    can_run_with_others: vec!["embedding".into(), "transcription".into()],
                    notes: "Best open-source reasoning. Run alone for max quality.".into(),
                },
                image: ModelRecommendation {
                    model_name: "flux.1-dev".into(),
                    quantization: None,
                    vram_needed_gb: 24.0,
                    quality_score: 0.93,
                    estimated_speed: "~3-5 min/image at 100 steps".into(),
                    can_run_with_others: vec!["voice".into(), "music".into(), "embedding".into()],
                    notes: "FP16, 100 diffusion steps for max quality.".into(),
                },
                voice: ModelRecommendation {
                    model_name: "f5-tts".into(),
                    quantization: None,
                    vram_needed_gb: 4.0,
                    quality_score: 0.82,
                    estimated_speed: "~30s per line".into(),
                    can_run_with_others: vec![
                        "image".into(),
                        "music".into(),
                        "embedding".into(),
                        "model3d".into(),
                    ],
                    notes: "Best open-source TTS. 15s reference audio for cloning.".into(),
                },
                music: ModelRecommendation {
                    model_name: "musicgen-large".into(),
                    quantization: None,
                    vram_needed_gb: 8.0,
                    quality_score: 0.65,
                    estimated_speed: "~2 min per 30s track".into(),
                    can_run_with_others: vec!["voice".into(), "embedding".into()],
                    notes: "Only viable local music gen. Acceptable for transitions.".into(),
                },
                model3d: ModelRecommendation {
                    model_name: "trellis".into(),
                    quantization: None,
                    vram_needed_gb: 16.0,
                    quality_score: 0.78,
                    estimated_speed: "~1-2 min per model".into(),
                    can_run_with_others: vec!["voice".into(), "embedding".into()],
                    notes: "Best open-source 3D gen. Game-ready topology.".into(),
                },
                embedding: ModelRecommendation {
                    model_name: "nomic-embed-text".into(),
                    quantization: None,
                    vram_needed_gb: 1.0,
                    quality_score: 0.92,
                    estimated_speed: "instant".into(),
                    can_run_with_others: vec![
                        "llm".into(),
                        "image".into(),
                        "voice".into(),
                        "music".into(),
                        "model3d".into(),
                    ],
                    notes: "Always runs alongside other models.".into(),
                },
                transcription: ModelRecommendation {
                    model_name: "whisper-large-v3".into(),
                    quantization: None,
                    vram_needed_gb: 3.0,
                    quality_score: 1.0,
                    estimated_speed: "~10x realtime".into(),
                    can_run_with_others: vec![
                        "llm".into(),
                        "image".into(),
                        "voice".into(),
                        "embedding".into(),
                    ],
                    notes: "Same quality as API. Already installed.".into(),
                },
            },
        }
    }

    fn m1_pro_32() -> Self {
        Self {
            name: "Apple M1 Pro 32GB".into(),
            gpu_vram_gb: 32.0,
            system_ram_gb: 32.0,
            unified_memory: true,
            gpu_type: GpuType::AppleSilicon {
                chip: "M1 Pro".into(),
                gpu_cores: 16,
            },
            recommended_models: RecommendedModels {
                llm: ModelRecommendation {
                    model_name: "qwen2.5:32b-instruct-q4_K_M".into(),
                    quantization: Some("Q4_K_M".into()),
                    vram_needed_gb: 20.0,
                    quality_score: 0.82,
                    estimated_speed: "~12 tok/s".into(),
                    can_run_with_others: vec!["embedding".into(), "transcription".into()],
                    notes: "32B at Q4 fits well. Run alone for best speed.".into(),
                },
                image: ModelRecommendation {
                    model_name: "flux.1-schnell".into(),
                    quantization: Some("FP16".into()),
                    vram_needed_gb: 12.0,
                    quality_score: 0.85,
                    estimated_speed: "~30s/image at 4 steps".into(),
                    can_run_with_others: vec!["voice".into(), "embedding".into()],
                    notes: "Schnell is faster. Dev might be too slow on M1 Pro.".into(),
                },
                voice: ModelRecommendation {
                    model_name: "f5-tts".into(),
                    quantization: None,
                    vram_needed_gb: 4.0,
                    quality_score: 0.82,
                    estimated_speed: "~45s per line".into(),
                    can_run_with_others: vec!["image".into(), "embedding".into()],
                    notes: "Runs well on M1 Pro.".into(),
                },
                music: ModelRecommendation {
                    model_name: "musicgen-medium".into(),
                    quantization: None,
                    vram_needed_gb: 4.0,
                    quality_score: 0.50,
                    estimated_speed: "~3 min per 30s".into(),
                    can_run_with_others: vec!["voice".into(), "embedding".into()],
                    notes: "Medium variant to save VRAM. Lower quality than Large.".into(),
                },
                model3d: ModelRecommendation {
                    model_name: "triposr".into(),
                    quantization: None,
                    vram_needed_gb: 8.0,
                    quality_score: 0.65,
                    estimated_speed: "~30s per model".into(),
                    can_run_with_others: vec!["voice".into(), "embedding".into()],
                    notes: "TripoSR over Trellis to save VRAM.".into(),
                },
                embedding: ModelRecommendation {
                    model_name: "nomic-embed-text".into(),
                    quantization: None,
                    vram_needed_gb: 1.0,
                    quality_score: 0.92,
                    estimated_speed: "instant".into(),
                    can_run_with_others: vec![
                        "llm".into(),
                        "image".into(),
                        "voice".into(),
                        "music".into(),
                        "model3d".into(),
                    ],
                    notes: "Always available.".into(),
                },
                transcription: ModelRecommendation {
                    model_name: "whisper-large-v3".into(),
                    quantization: None,
                    vram_needed_gb: 3.0,
                    quality_score: 1.0,
                    estimated_speed: "~5x realtime".into(),
                    can_run_with_others: vec!["embedding".into()],
                    notes: "Slower than M4 Max but still good.".into(),
                },
            },
        }
    }

    fn nvidia_5900x_3080ti() -> Self {
        Self {
            name: "AMD 5900X + RTX 3080 Ti".into(),
            gpu_vram_gb: 12.0,
            system_ram_gb: 64.0,
            unified_memory: false,
            gpu_type: GpuType::NvidiaDesktop {
                model: "RTX 3080 Ti".into(),
                cuda_cores: 10240,
            },
            recommended_models: Self::nvidia_12gb_models(),
        }
    }

    fn nvidia_12gb_models() -> RecommendedModels {
        RecommendedModels {
            llm: ModelRecommendation {
                model_name: "qwen2.5:14b-instruct-q8_0".into(),
                quantization: Some("Q8_0".into()),
                vram_needed_gb: 10.0,
                quality_score: 0.75,
                estimated_speed: "~25 tok/s (CUDA)".into(),
                can_run_with_others: vec![],
                notes: "14B at Q8 fills 12GB. For 70B use CPU offloading (very slow).".into(),
            },
            image: ModelRecommendation {
                model_name: "flux.1-schnell".into(),
                quantization: Some("FP16".into()),
                vram_needed_gb: 12.0,
                quality_score: 0.85,
                estimated_speed: "~5s/image (CUDA is fast)".into(),
                can_run_with_others: vec![],
                notes: "Fills VRAM. CUDA makes it fast though. Swap with LLM.".into(),
            },
            voice: ModelRecommendation {
                model_name: "f5-tts".into(),
                quantization: None,
                vram_needed_gb: 4.0,
                quality_score: 0.82,
                estimated_speed: "~15s per line (CUDA)".into(),
                can_run_with_others: vec!["music".into(), "embedding".into()],
                notes: "Fast on CUDA. Can share VRAM with smaller models.".into(),
            },
            music: ModelRecommendation {
                model_name: "musicgen-large".into(),
                quantization: None,
                vram_needed_gb: 8.0,
                quality_score: 0.65,
                estimated_speed: "~30s per 30s track (CUDA)".into(),
                can_run_with_others: vec!["voice".into()],
                notes: "CUDA acceleration makes Large viable.".into(),
            },
            model3d: ModelRecommendation {
                model_name: "triposr".into(),
                quantization: None,
                vram_needed_gb: 8.0,
                quality_score: 0.65,
                estimated_speed: "~10s per model (CUDA)".into(),
                can_run_with_others: vec!["voice".into()],
                notes: "TripoSR fits. Trellis (16GB) won't fit.".into(),
            },
            embedding: ModelRecommendation {
                model_name: "nomic-embed-text".into(),
                quantization: None,
                vram_needed_gb: 1.0,
                quality_score: 0.92,
                estimated_speed: "instant".into(),
                can_run_with_others: vec![
                    "llm".into(),
                    "voice".into(),
                    "music".into(),
                    "model3d".into(),
                ],
                notes: "Tiny, always fits.".into(),
            },
            transcription: ModelRecommendation {
                model_name: "whisper-large-v3".into(),
                quantization: None,
                vram_needed_gb: 3.0,
                quality_score: 1.0,
                estimated_speed: "~20x realtime (CUDA)".into(),
                can_run_with_others: vec!["embedding".into()],
                notes: "Fastest Whisper on CUDA. Excellent.".into(),
            },
        }
    }

    fn nvidia_24gb_models() -> RecommendedModels {
        RecommendedModels {
            llm: ModelRecommendation {
                model_name: "qwen2.5:32b-instruct-q6_K".into(),
                quantization: Some("Q6_K".into()),
                vram_needed_gb: 22.0,
                quality_score: 0.85,
                estimated_speed: "~20 tok/s (CUDA)".into(),
                can_run_with_others: vec![],
                notes: "32B at Q6 fits in 24GB. Excellent speed on CUDA.".into(),
            },
            image: ModelRecommendation {
                model_name: "flux.1-dev".into(),
                quantization: Some("FP16".into()),
                vram_needed_gb: 24.0,
                quality_score: 0.93,
                estimated_speed: "~30s/image (CUDA)".into(),
                can_run_with_others: vec![],
                notes: "Fills VRAM but CUDA is fast. Swap with LLM.".into(),
            },
            voice: ModelRecommendation {
                model_name: "f5-tts".into(),
                quantization: None,
                vram_needed_gb: 4.0,
                quality_score: 0.82,
                estimated_speed: "~10s per line (CUDA)".into(),
                can_run_with_others: vec!["music".into(), "model3d".into(), "embedding".into()],
                notes: "Fast on CUDA with room to spare.".into(),
            },
            music: ModelRecommendation {
                model_name: "musicgen-large".into(),
                quantization: None,
                vram_needed_gb: 8.0,
                quality_score: 0.65,
                estimated_speed: "~20s per 30s track (CUDA)".into(),
                can_run_with_others: vec!["voice".into(), "embedding".into()],
                notes: "Large variant fits with room for others.".into(),
            },
            model3d: ModelRecommendation {
                model_name: "trellis".into(),
                quantization: None,
                vram_needed_gb: 16.0,
                quality_score: 0.78,
                estimated_speed: "~30s per model (CUDA)".into(),
                can_run_with_others: vec!["voice".into(), "embedding".into()],
                notes: "Trellis fits in 24GB. Best open-source 3D.".into(),
            },
            embedding: ModelRecommendation {
                model_name: "nomic-embed-text".into(),
                quantization: None,
                vram_needed_gb: 1.0,
                quality_score: 0.92,
                estimated_speed: "instant".into(),
                can_run_with_others: vec![
                    "llm".into(),
                    "voice".into(),
                    "music".into(),
                    "model3d".into(),
                ],
                notes: "Tiny, always fits.".into(),
            },
            transcription: ModelRecommendation {
                model_name: "whisper-large-v3".into(),
                quantization: None,
                vram_needed_gb: 3.0,
                quality_score: 1.0,
                estimated_speed: "~20x realtime (CUDA)".into(),
                can_run_with_others: vec!["embedding".into()],
                notes: "Fastest Whisper on CUDA.".into(),
            },
        }
    }

    fn nvidia_8gb_laptop_models() -> RecommendedModels {
        RecommendedModels {
            llm: ModelRecommendation {
                model_name: "qwen2.5:7b-instruct-q8_0".into(),
                quantization: Some("Q8_0".into()),
                vram_needed_gb: 6.0,
                quality_score: 0.65,
                estimated_speed: "~30 tok/s (CUDA)".into(),
                can_run_with_others: vec!["embedding".into()],
                notes: "7B at Q8 is the max for 8GB. Decent speed.".into(),
            },
            image: ModelRecommendation {
                model_name: "flux.1-schnell".into(),
                quantization: Some("Q4".into()),
                vram_needed_gb: 6.0,
                quality_score: 0.75,
                estimated_speed: "~10s/image (CUDA)".into(),
                can_run_with_others: vec![],
                notes: "Quantized Schnell to fit in 8GB.".into(),
            },
            voice: ModelRecommendation {
                model_name: "f5-tts".into(),
                quantization: None,
                vram_needed_gb: 4.0,
                quality_score: 0.82,
                estimated_speed: "~20s per line (CUDA)".into(),
                can_run_with_others: vec!["embedding".into()],
                notes: "Fits but tight on VRAM.".into(),
            },
            music: ModelRecommendation {
                model_name: "musicgen-small".into(),
                quantization: None,
                vram_needed_gb: 2.0,
                quality_score: 0.40,
                estimated_speed: "~2 min per 30s (CUDA)".into(),
                can_run_with_others: vec!["voice".into(), "embedding".into()],
                notes: "Small variant for limited VRAM. Low quality.".into(),
            },
            model3d: ModelRecommendation {
                model_name: "triposr".into(),
                quantization: None,
                vram_needed_gb: 8.0,
                quality_score: 0.65,
                estimated_speed: "~15s per model (CUDA)".into(),
                can_run_with_others: vec![],
                notes: "Fills VRAM. Run alone.".into(),
            },
            embedding: ModelRecommendation {
                model_name: "nomic-embed-text".into(),
                quantization: None,
                vram_needed_gb: 1.0,
                quality_score: 0.92,
                estimated_speed: "instant".into(),
                can_run_with_others: vec!["llm".into(), "voice".into(), "music".into()],
                notes: "Tiny, always fits.".into(),
            },
            transcription: ModelRecommendation {
                model_name: "whisper-medium".into(),
                quantization: None,
                vram_needed_gb: 2.0,
                quality_score: 0.90,
                estimated_speed: "~15x realtime (CUDA)".into(),
                can_run_with_others: vec!["embedding".into()],
                notes: "Medium variant to save VRAM. Still good quality.".into(),
            },
        }
    }

    fn cpu_only_models() -> RecommendedModels {
        RecommendedModels {
            llm: ModelRecommendation {
                model_name: "qwen2.5:3b-instruct-q4_K_M".into(),
                quantization: Some("Q4_K_M".into()),
                vram_needed_gb: 0.0,
                quality_score: 0.45,
                estimated_speed: "~5 tok/s (CPU)".into(),
                can_run_with_others: vec![],
                notes: "Tiny model for CPU inference. Very limited quality.".into(),
            },
            image: ModelRecommendation {
                model_name: "none".into(),
                quantization: None,
                vram_needed_gb: 0.0,
                quality_score: 0.0,
                estimated_speed: "n/a".into(),
                can_run_with_others: vec![],
                notes: "No viable local image gen without GPU.".into(),
            },
            voice: ModelRecommendation {
                model_name: "piper-tts".into(),
                quantization: None,
                vram_needed_gb: 0.0,
                quality_score: 0.50,
                estimated_speed: "~2s per line (CPU)".into(),
                can_run_with_others: vec!["embedding".into()],
                notes: "Lightweight CPU TTS. Lower quality than F5.".into(),
            },
            music: ModelRecommendation {
                model_name: "none".into(),
                quantization: None,
                vram_needed_gb: 0.0,
                quality_score: 0.0,
                estimated_speed: "n/a".into(),
                can_run_with_others: vec![],
                notes: "No viable local music gen without GPU.".into(),
            },
            model3d: ModelRecommendation {
                model_name: "none".into(),
                quantization: None,
                vram_needed_gb: 0.0,
                quality_score: 0.0,
                estimated_speed: "n/a".into(),
                can_run_with_others: vec![],
                notes: "No viable local 3D gen without GPU.".into(),
            },
            embedding: ModelRecommendation {
                model_name: "nomic-embed-text".into(),
                quantization: None,
                vram_needed_gb: 0.0,
                quality_score: 0.92,
                estimated_speed: "~1s (CPU)".into(),
                can_run_with_others: vec!["llm".into(), "voice".into()],
                notes: "Works fine on CPU.".into(),
            },
            transcription: ModelRecommendation {
                model_name: "whisper-base".into(),
                quantization: None,
                vram_needed_gb: 0.0,
                quality_score: 0.70,
                estimated_speed: "~2x realtime (CPU)".into(),
                can_run_with_others: vec!["embedding".into()],
                notes: "Base model for CPU. Slower and less accurate.".into(),
            },
        }
    }

    fn cpu_only_fallback() -> Self {
        Self {
            name: "CPU Only (fallback)".into(),
            gpu_vram_gb: 0.0,
            system_ram_gb: 8.0,
            unified_memory: false,
            gpu_type: GpuType::CpuOnly,
            recommended_models: Self::cpu_only_models(),
        }
    }

    fn apple_silicon_preset(
        name: &str,
        memory_gigabytes: f64,
        chip: &str,
        gpu_cores: u32,
        models: RecommendedModels,
    ) -> Self {
        Self {
            name: name.into(),
            gpu_vram_gb: memory_gigabytes,
            system_ram_gb: memory_gigabytes,
            unified_memory: true,
            gpu_type: GpuType::AppleSilicon {
                chip: chip.into(),
                gpu_cores,
            },
            recommended_models: models,
        }
    }

    fn nvidia_desktop_preset(
        name: &str,
        vram_gb: f64,
        system_ram_gb: f64,
        gpu_model: &str,
        cuda_cores: u32,
        models: RecommendedModels,
    ) -> Self {
        Self {
            name: name.into(),
            gpu_vram_gb: vram_gb,
            system_ram_gb,
            unified_memory: false,
            gpu_type: GpuType::NvidiaDesktop {
                model: gpu_model.into(),
                cuda_cores,
            },
            recommended_models: models,
        }
    }

    fn ultra_apple_models() -> RecommendedModels {
        RecommendedModels {
            llm: ModelRecommendation {
                model_name: "qwen2.5:72b-instruct-q8_0".into(),
                quantization: Some("Q8_0".into()),
                vram_needed_gb: 75.0,
                quality_score: 0.95,
                estimated_speed: "~6 tok/s".into(),
                can_run_with_others: vec!["embedding".into(), "transcription".into()],
                notes: "72B at Q8 with room to spare. Highest local quality.".into(),
            },
            image: ModelRecommendation {
                model_name: "flux.1-dev".into(),
                quantization: None,
                vram_needed_gb: 24.0,
                quality_score: 0.93,
                estimated_speed: "~2-4 min/image".into(),
                can_run_with_others: vec![
                    "voice".into(),
                    "music".into(),
                    "model3d".into(),
                    "embedding".into(),
                ],
                notes: "Full FP16. Can run concurrently with most models.".into(),
            },
            voice: ModelRecommendation {
                model_name: "f5-tts".into(),
                quantization: None,
                vram_needed_gb: 4.0,
                quality_score: 0.82,
                estimated_speed: "~25s per line".into(),
                can_run_with_others: vec![
                    "image".into(),
                    "music".into(),
                    "model3d".into(),
                    "embedding".into(),
                ],
                notes: "Plenty of room to run alongside anything.".into(),
            },
            music: ModelRecommendation {
                model_name: "musicgen-large".into(),
                quantization: None,
                vram_needed_gb: 8.0,
                quality_score: 0.65,
                estimated_speed: "~90s per 30s track".into(),
                can_run_with_others: vec![
                    "voice".into(),
                    "image".into(),
                    "model3d".into(),
                    "embedding".into(),
                ],
                notes: "Large variant with plenty of VRAM headroom.".into(),
            },
            model3d: ModelRecommendation {
                model_name: "trellis".into(),
                quantization: None,
                vram_needed_gb: 16.0,
                quality_score: 0.78,
                estimated_speed: "~1 min per model".into(),
                can_run_with_others: vec!["voice".into(), "music".into(), "embedding".into()],
                notes: "Best open-source 3D gen. Lots of VRAM to spare.".into(),
            },
            embedding: ModelRecommendation {
                model_name: "nomic-embed-text".into(),
                quantization: None,
                vram_needed_gb: 1.0,
                quality_score: 0.92,
                estimated_speed: "instant".into(),
                can_run_with_others: vec![
                    "llm".into(),
                    "image".into(),
                    "voice".into(),
                    "music".into(),
                    "model3d".into(),
                ],
                notes: "Always runs alongside other models.".into(),
            },
            transcription: ModelRecommendation {
                model_name: "whisper-large-v3".into(),
                quantization: None,
                vram_needed_gb: 3.0,
                quality_score: 1.0,
                estimated_speed: "~8x realtime".into(),
                can_run_with_others: vec![
                    "llm".into(),
                    "image".into(),
                    "voice".into(),
                    "embedding".into(),
                ],
                notes: "Full quality Whisper.".into(),
            },
        }
    }

    fn large_apple_models() -> RecommendedModels {
        RecommendedModels {
            llm: ModelRecommendation {
                model_name: "qwen2.5:72b-instruct-q4_K_M".into(),
                quantization: Some("Q4_K_M".into()),
                vram_needed_gb: 42.0,
                quality_score: 0.90,
                estimated_speed: "~10 tok/s".into(),
                can_run_with_others: vec!["embedding".into(), "transcription".into()],
                notes: "72B at Q4_K_M. Good balance of quality and speed.".into(),
            },
            image: ModelRecommendation {
                model_name: "flux.1-dev".into(),
                quantization: None,
                vram_needed_gb: 24.0,
                quality_score: 0.93,
                estimated_speed: "~3-5 min/image".into(),
                can_run_with_others: vec!["voice".into(), "music".into(), "embedding".into()],
                notes: "FP16 Dev variant for best quality.".into(),
            },
            voice: ModelRecommendation {
                model_name: "f5-tts".into(),
                quantization: None,
                vram_needed_gb: 4.0,
                quality_score: 0.82,
                estimated_speed: "~30s per line".into(),
                can_run_with_others: vec![
                    "image".into(),
                    "music".into(),
                    "embedding".into(),
                    "model3d".into(),
                ],
                notes: "Best open-source TTS.".into(),
            },
            music: ModelRecommendation {
                model_name: "musicgen-large".into(),
                quantization: None,
                vram_needed_gb: 8.0,
                quality_score: 0.65,
                estimated_speed: "~2 min per 30s track".into(),
                can_run_with_others: vec!["voice".into(), "embedding".into()],
                notes: "Large variant fits well.".into(),
            },
            model3d: ModelRecommendation {
                model_name: "trellis".into(),
                quantization: None,
                vram_needed_gb: 16.0,
                quality_score: 0.78,
                estimated_speed: "~1-2 min per model".into(),
                can_run_with_others: vec!["voice".into(), "embedding".into()],
                notes: "Best open-source 3D gen.".into(),
            },
            embedding: ModelRecommendation {
                model_name: "nomic-embed-text".into(),
                quantization: None,
                vram_needed_gb: 1.0,
                quality_score: 0.92,
                estimated_speed: "instant".into(),
                can_run_with_others: vec![
                    "llm".into(),
                    "image".into(),
                    "voice".into(),
                    "music".into(),
                    "model3d".into(),
                ],
                notes: "Always available.".into(),
            },
            transcription: ModelRecommendation {
                model_name: "whisper-large-v3".into(),
                quantization: None,
                vram_needed_gb: 3.0,
                quality_score: 1.0,
                estimated_speed: "~8x realtime".into(),
                can_run_with_others: vec![
                    "llm".into(),
                    "image".into(),
                    "voice".into(),
                    "embedding".into(),
                ],
                notes: "Full quality Whisper.".into(),
            },
        }
    }

    fn mid_apple_models() -> RecommendedModels {
        RecommendedModels {
            llm: ModelRecommendation {
                model_name: "qwen2.5:32b-instruct-q6_K".into(),
                quantization: Some("Q6_K".into()),
                vram_needed_gb: 25.0,
                quality_score: 0.85,
                estimated_speed: "~10 tok/s".into(),
                can_run_with_others: vec!["embedding".into(), "transcription".into()],
                notes: "32B at Q6_K fits well in 32-36GB.".into(),
            },
            image: ModelRecommendation {
                model_name: "flux.1-schnell".into(),
                quantization: Some("FP16".into()),
                vram_needed_gb: 12.0,
                quality_score: 0.85,
                estimated_speed: "~20s/image at 4 steps".into(),
                can_run_with_others: vec!["voice".into(), "embedding".into()],
                notes: "Schnell for speed. Dev is too large for this tier.".into(),
            },
            voice: ModelRecommendation {
                model_name: "f5-tts".into(),
                quantization: None,
                vram_needed_gb: 4.0,
                quality_score: 0.82,
                estimated_speed: "~35s per line".into(),
                can_run_with_others: vec!["image".into(), "embedding".into()],
                notes: "Runs well at this tier.".into(),
            },
            music: ModelRecommendation {
                model_name: "musicgen-medium".into(),
                quantization: None,
                vram_needed_gb: 4.0,
                quality_score: 0.50,
                estimated_speed: "~3 min per 30s".into(),
                can_run_with_others: vec!["voice".into(), "embedding".into()],
                notes: "Medium to save VRAM.".into(),
            },
            model3d: ModelRecommendation {
                model_name: "triposr".into(),
                quantization: None,
                vram_needed_gb: 8.0,
                quality_score: 0.65,
                estimated_speed: "~30s per model".into(),
                can_run_with_others: vec!["voice".into(), "embedding".into()],
                notes: "TripoSR over Trellis to save VRAM.".into(),
            },
            embedding: ModelRecommendation {
                model_name: "nomic-embed-text".into(),
                quantization: None,
                vram_needed_gb: 1.0,
                quality_score: 0.92,
                estimated_speed: "instant".into(),
                can_run_with_others: vec![
                    "llm".into(),
                    "image".into(),
                    "voice".into(),
                    "music".into(),
                    "model3d".into(),
                ],
                notes: "Always available.".into(),
            },
            transcription: ModelRecommendation {
                model_name: "whisper-large-v3".into(),
                quantization: None,
                vram_needed_gb: 3.0,
                quality_score: 1.0,
                estimated_speed: "~6x realtime".into(),
                can_run_with_others: vec!["embedding".into()],
                notes: "Full quality Whisper.".into(),
            },
        }
    }

    fn small_apple_models() -> RecommendedModels {
        RecommendedModels {
            llm: ModelRecommendation {
                model_name: "qwen2.5:14b-instruct-q6_K".into(),
                quantization: Some("Q6_K".into()),
                vram_needed_gb: 14.0,
                quality_score: 0.78,
                estimated_speed: "~15 tok/s".into(),
                can_run_with_others: vec!["embedding".into(), "transcription".into()],
                notes: "14B at Q6 fits well in 24GB unified.".into(),
            },
            image: ModelRecommendation {
                model_name: "flux.1-schnell".into(),
                quantization: Some("FP16".into()),
                vram_needed_gb: 12.0,
                quality_score: 0.85,
                estimated_speed: "~25s/image at 4 steps".into(),
                can_run_with_others: vec!["embedding".into()],
                notes: "Schnell for speed on limited VRAM.".into(),
            },
            voice: ModelRecommendation {
                model_name: "f5-tts".into(),
                quantization: None,
                vram_needed_gb: 4.0,
                quality_score: 0.82,
                estimated_speed: "~40s per line".into(),
                can_run_with_others: vec!["embedding".into()],
                notes: "Tight fit but works.".into(),
            },
            music: ModelRecommendation {
                model_name: "musicgen-small".into(),
                quantization: None,
                vram_needed_gb: 2.0,
                quality_score: 0.40,
                estimated_speed: "~3 min per 30s".into(),
                can_run_with_others: vec!["voice".into(), "embedding".into()],
                notes: "Small variant for limited VRAM.".into(),
            },
            model3d: ModelRecommendation {
                model_name: "triposr".into(),
                quantization: None,
                vram_needed_gb: 8.0,
                quality_score: 0.65,
                estimated_speed: "~30s per model".into(),
                can_run_with_others: vec!["embedding".into()],
                notes: "TripoSR fits. Run alone for best results.".into(),
            },
            embedding: ModelRecommendation {
                model_name: "nomic-embed-text".into(),
                quantization: None,
                vram_needed_gb: 1.0,
                quality_score: 0.92,
                estimated_speed: "instant".into(),
                can_run_with_others: vec![
                    "llm".into(),
                    "image".into(),
                    "voice".into(),
                    "music".into(),
                    "model3d".into(),
                ],
                notes: "Always available.".into(),
            },
            transcription: ModelRecommendation {
                model_name: "whisper-large-v3".into(),
                quantization: None,
                vram_needed_gb: 3.0,
                quality_score: 1.0,
                estimated_speed: "~4x realtime".into(),
                can_run_with_others: vec!["embedding".into()],
                notes: "Still fits in 24GB alongside embedding.".into(),
            },
        }
    }

    fn tiny_apple_models() -> RecommendedModels {
        RecommendedModels {
            llm: ModelRecommendation {
                model_name: "qwen2.5:7b-instruct-q6_K".into(),
                quantization: Some("Q6_K".into()),
                vram_needed_gb: 7.0,
                quality_score: 0.65,
                estimated_speed: "~20 tok/s".into(),
                can_run_with_others: vec!["embedding".into(), "transcription".into()],
                notes: "7B at Q6 is the max for 16GB. Limited quality.".into(),
            },
            image: ModelRecommendation {
                model_name: "flux.1-schnell".into(),
                quantization: Some("Q4".into()),
                vram_needed_gb: 6.0,
                quality_score: 0.75,
                estimated_speed: "~30s/image".into(),
                can_run_with_others: vec!["embedding".into()],
                notes: "Quantized Schnell to fit in 16GB.".into(),
            },
            voice: ModelRecommendation {
                model_name: "f5-tts".into(),
                quantization: None,
                vram_needed_gb: 4.0,
                quality_score: 0.82,
                estimated_speed: "~50s per line".into(),
                can_run_with_others: vec!["embedding".into()],
                notes: "Tight fit but works.".into(),
            },
            music: ModelRecommendation {
                model_name: "musicgen-small".into(),
                quantization: None,
                vram_needed_gb: 2.0,
                quality_score: 0.40,
                estimated_speed: "~4 min per 30s".into(),
                can_run_with_others: vec!["embedding".into()],
                notes: "Small variant. Low quality but fits.".into(),
            },
            model3d: ModelRecommendation {
                model_name: "triposr".into(),
                quantization: None,
                vram_needed_gb: 8.0,
                quality_score: 0.65,
                estimated_speed: "~45s per model".into(),
                can_run_with_others: vec![],
                notes: "Fills most of VRAM. Run alone.".into(),
            },
            embedding: ModelRecommendation {
                model_name: "nomic-embed-text".into(),
                quantization: None,
                vram_needed_gb: 1.0,
                quality_score: 0.92,
                estimated_speed: "instant".into(),
                can_run_with_others: vec![
                    "llm".into(),
                    "image".into(),
                    "voice".into(),
                    "music".into(),
                ],
                notes: "Always fits.".into(),
            },
            transcription: ModelRecommendation {
                model_name: "whisper-medium".into(),
                quantization: None,
                vram_needed_gb: 2.0,
                quality_score: 0.90,
                estimated_speed: "~4x realtime".into(),
                can_run_with_others: vec!["embedding".into()],
                notes: "Medium variant for 16GB.".into(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_m4_max_preset_runs_the_largest_local_models() {
        let profile = MachineProfile::from_preset("m4-max-64").unwrap();
        assert_eq!(profile.name, "Apple M4 Max 64GB");
        assert_eq!(profile.gpu_vram_gb, 64.0);
        assert!(profile.unified_memory);
        assert_eq!(
            profile.recommended_models.llm.model_name,
            "qwen2.5:72b-instruct-q6_K"
        );
        assert_eq!(profile.recommended_models.llm.vram_needed_gb, 55.0);
        assert!((profile.recommended_models.llm.quality_score - 0.92).abs() < f64::EPSILON);
        assert_eq!(profile.recommended_models.image.model_name, "flux.1-dev");
        assert_eq!(profile.recommended_models.voice.model_name, "f5-tts");
        assert_eq!(
            profile.recommended_models.music.model_name,
            "musicgen-large"
        );
        assert_eq!(profile.recommended_models.model3d.model_name, "trellis");
        assert_eq!(
            profile.recommended_models.embedding.model_name,
            "nomic-embed-text"
        );
        assert_eq!(
            profile.recommended_models.transcription.model_name,
            "whisper-large-v3"
        );
        match &profile.gpu_type {
            GpuType::AppleSilicon { chip, gpu_cores } => {
                assert_eq!(chip, "M4 Max");
                assert_eq!(*gpu_cores, 40);
            }
            _ => panic!("expected AppleSilicon"),
        }
    }

    #[test]
    fn the_m1_pro_preset_steps_down_to_smaller_models() {
        let profile = MachineProfile::from_preset("m1-pro-32").unwrap();
        assert_eq!(profile.name, "Apple M1 Pro 32GB");
        assert_eq!(profile.gpu_vram_gb, 32.0);
        assert!(profile.unified_memory);
        assert_eq!(
            profile.recommended_models.llm.model_name,
            "qwen2.5:32b-instruct-q4_K_M"
        );
        assert!(profile.recommended_models.llm.quality_score < 0.92);
        assert_eq!(
            profile.recommended_models.image.model_name,
            "flux.1-schnell"
        );
        assert_eq!(
            profile.recommended_models.music.model_name,
            "musicgen-medium"
        );
        assert_eq!(profile.recommended_models.model3d.model_name, "triposr");
    }

    #[test]
    fn a_discrete_gpu_preset_reports_its_card_and_its_vram() {
        let profile = MachineProfile::from_preset("5900x-3080ti").unwrap();
        assert_eq!(profile.name, "AMD 5900X + RTX 3080 Ti");
        assert_eq!(profile.gpu_vram_gb, 12.0);
        assert_eq!(profile.system_ram_gb, 64.0);
        assert!(!profile.unified_memory);
        assert_eq!(
            profile.recommended_models.llm.model_name,
            "qwen2.5:14b-instruct-q8_0"
        );
        assert!(
            profile
                .recommended_models
                .llm
                .estimated_speed
                .contains("CUDA")
        );
        match &profile.gpu_type {
            GpuType::NvidiaDesktop { model, cuda_cores } => {
                assert_eq!(model, "RTX 3080 Ti");
                assert_eq!(*cuda_cores, 10240);
            }
            _ => panic!("expected NvidiaDesktop"),
        }
    }

    #[test]
    fn an_unknown_preset_is_none_rather_than_a_guess() {
        assert!(MachineProfile::from_preset("nonexistent-machine").is_none());
        assert!(MachineProfile::from_preset("").is_none());
        assert!(MachineProfile::from_preset("m99-ultra-1024").is_none());
    }

    #[test]
    fn every_listed_preset_resolves() {
        let presets = MachineProfile::available_presets();
        assert_eq!(presets.len(), 15);
        let slugs: Vec<&str> = presets.iter().map(|(s, _)| *s).collect();
        assert!(slugs.contains(&"m4-max-64"));
        assert!(slugs.contains(&"m1-pro-32"));
        assert!(slugs.contains(&"5900x-3080ti"));
        assert!(slugs.contains(&"cpu-only"));
        assert!(slugs.contains(&"laptop-4060"));
        for (slug, desc) in &presets {
            assert!(!slug.is_empty());
            assert!(!desc.is_empty());
            assert!(
                MachineProfile::from_preset(slug).is_some(),
                "preset '{}' not found",
                slug
            );
        }
    }

    #[test]
    fn unified_memory_reports_the_whole_pool() {
        let profile = MachineProfile::from_preset("m4-max-64").unwrap();
        assert!(profile.unified_memory);
        assert!((profile.effective_vram() - 64.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_discrete_gpu_reports_its_own_vram_not_system_ram() {
        let profile = MachineProfile::from_preset("5900x-3080ti").unwrap();
        assert!(!profile.unified_memory);
        assert!((profile.effective_vram() - 12.0).abs() < f64::EPSILON);
        assert!((profile.system_ram_gb - 64.0).abs() < f64::EPSILON);
    }

    #[test]
    fn unified_memory_reserves_a_fifth_for_the_operating_system() {
        let unified = MachineProfile::from_preset("m4-max-64").unwrap();
        assert!((unified.maximum_concurrent_vram() - 64.0 * 0.80).abs() < f64::EPSILON);

        let discrete = MachineProfile::from_preset("5900x-3080ti").unwrap();
        assert!((discrete.maximum_concurrent_vram() - 12.0).abs() < f64::EPSILON);
    }

    #[test]
    fn nvidia_smi_output_for_several_gpus_describes_the_first() {
        assert_eq!(
            MachineProfile::first_gpu(
                "NVIDIA GeForce RTX 4090, 24564\nNVIDIA GeForce RTX 3090, 24576\n"
            ),
            Some(("NVIDIA GeForce RTX 4090".to_string(), 24.0))
        );
        assert_eq!(
            MachineProfile::first_gpu("NVIDIA RTX A6000, 49140\n"),
            Some(("NVIDIA RTX A6000".to_string(), 48.0))
        );
        assert_eq!(MachineProfile::first_gpu(""), None);
        assert_eq!(MachineProfile::first_gpu("No devices were found"), None);
    }

    #[test]
    fn installed_memory_is_reported_as_sold() {
        const GIGABYTE: u64 = 1024 * 1024 * 1024;
        for gigabytes in [8, 16, 18, 24, 36, 48, 64, 96, 128, 192] {
            assert_eq!(
                MachineProfile::memory_gigabytes(gigabytes * GIGABYTE),
                gigabytes as f64,
                "{gigabytes} GB"
            );
        }
    }

    #[tokio::test]
    async fn detection_off_the_runtime_produces_a_usable_profile() {
        let profile = MachineProfile::detect().await;
        assert!(!profile.name.is_empty());
        assert!(profile.system_ram_gb >= 0.0);
    }

    #[test]
    fn detection_always_produces_a_usable_profile() {
        let profile = MachineProfile::detect_blocking();
        assert!(!profile.name.is_empty());
        assert!(profile.recommended_models.embedding.quality_score > 0.0);
    }

    #[test]
    fn a_profile_round_trips_through_json() {
        let profile = MachineProfile::from_preset("m4-max-64").unwrap();
        let json = serde_json::to_string(&profile).unwrap();
        let roundtrip: MachineProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip.name, profile.name);
        assert_eq!(roundtrip.gpu_vram_gb, profile.gpu_vram_gb);
        assert_eq!(roundtrip.unified_memory, profile.unified_memory);
        assert_eq!(
            roundtrip.recommended_models.llm.model_name,
            profile.recommended_models.llm.model_name,
        );
    }

    #[test]
    fn every_gpu_type_round_trips() {
        let types = vec![
            GpuType::AppleSilicon {
                chip: "M4 Max".into(),
                gpu_cores: 40,
            },
            GpuType::NvidiaDesktop {
                model: "RTX 4090".into(),
                cuda_cores: 16384,
            },
            GpuType::NvidiaLaptop {
                model: "RTX 4060".into(),
                cuda_cores: 3072,
            },
            GpuType::AmdDesktop {
                model: "RX 7900 XTX".into(),
            },
            GpuType::IntelArc {
                model: "A770".into(),
            },
            GpuType::CpuOnly,
        ];
        for gpu in &types {
            let json = serde_json::to_string(gpu).unwrap();
            let roundtrip: GpuType = serde_json::from_str(&json).unwrap();
            let json2 = serde_json::to_string(&roundtrip).unwrap();
            assert_eq!(json, json2);
        }
    }

    #[test]
    fn every_recommendation_is_filled_in() {
        let profile = MachineProfile::from_preset("m4-max-64").unwrap();
        let rec = &profile.recommended_models.llm;
        assert!(rec.vram_needed_gb > 0.0);
        assert!(rec.quality_score > 0.0 && rec.quality_score <= 1.0);
        assert!(!rec.estimated_speed.is_empty());
        assert!(!rec.notes.is_empty());
    }

    #[test]
    fn a_cpu_only_machine_runs_almost_nothing_locally() {
        let profile = MachineProfile::from_preset("cpu-only").unwrap();
        assert_eq!(profile.gpu_vram_gb, 0.0);
        assert!(!profile.unified_memory);
        matches!(profile.gpu_type, GpuType::CpuOnly);
        assert_eq!(profile.recommended_models.image.model_name, "none");
        assert_eq!(profile.recommended_models.music.model_name, "none");
        assert!(profile.recommended_models.llm.quality_score < 0.50);
    }

    #[test]
    fn the_laptop_preset_reports_a_laptop_gpu() {
        let profile = MachineProfile::from_preset("laptop-4060").unwrap();
        assert_eq!(profile.gpu_vram_gb, 8.0);
        assert!(!profile.unified_memory);
        match &profile.gpu_type {
            GpuType::NvidiaLaptop { model, cuda_cores } => {
                assert!(model.contains("4060"));
                assert_eq!(*cuda_cores, 3072);
            }
            _ => panic!("expected NvidiaLaptop"),
        }
    }
}
