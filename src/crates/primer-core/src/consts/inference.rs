//! Tunables for the embedded llama.cpp backend (Phase 1.1) and
//! inference-backend defaults shared across the CLI, GUI, and engine.

/// Default Anthropic model id used when the cloud backend is selected
/// without an explicit model (primary `--model` or `--fallback-model`).
pub const DEFAULT_CLOUD_MODEL: &str = "claude-sonnet-4-6";

/// `n_gpu_layers` value meaning "offload every layer to the GPU".
/// The default when a GPU passthrough feature (metal/cuda/vulkan) is
/// compiled in.
pub const LLAMACPP_GPU_LAYERS_ALL: i32 = -1;

/// `n_gpu_layers` value meaning "CPU only". The default on a plain
/// `llamacpp` (no-GPU) build.
pub const LLAMACPP_GPU_LAYERS_CPU: i32 = 0;

/// Default `n_ctx`. `0` tells llama.cpp to use the model's own trained
/// context length rather than imposing one.
pub const LLAMACPP_DEFAULT_N_CTX: u32 = 0;

/// Fixed RNG seed for the sampler so a given prompt + params is
/// reproducible across runs. (`GenerationParams` carries no seed.)
pub const LLAMACPP_DEFAULT_SAMPLER_SEED: u32 = 1234;
