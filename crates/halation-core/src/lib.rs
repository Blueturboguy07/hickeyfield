//! Halation core.
//!
//! Provider adapters, route resolution, the job engine, the preset compiler and
//! cost estimation. This crate deliberately has no Tauri dependency so it stays
//! unit-testable without linking a webview.

pub mod camera;
pub mod capability;
pub mod catalog;
pub mod clients;
pub mod compositor;
pub mod corpus;
pub mod cost;
pub mod engine;
pub mod enhance;
pub mod enhancer;
pub mod fal_catalogue;
pub mod fal_schema;
pub mod ffmpeg;
pub mod gaps;
pub mod job;
pub mod media;
pub mod onboarding;
pub mod preset;
pub mod prices;
pub mod provider;
pub mod recipe;
pub mod registry;
pub mod route;
pub mod use_case;

pub use camera::CameraTemplate;
pub use catalog::{Arity, FlagSpec, Modality, ModelSpec, ValueSpec};
pub use cost::{Billable, CostModel, Estimate};
pub use enhance::{
    EnhanceDecision, EnhanceInputs, EnhanceReason, JobType, PresetSelection, PromptParts,
    SentinelKind,
};
pub use job::{JobStatus, Phase};
pub use media::{BindError, Dialect, MediaRef, MediaRole, MediaSource, Uploader};
pub use onboarding::{all_provider_info, parse_env, ImportResult, ProviderInfo};
pub use preset::{Category, PresetFamily, PresetVariant, Requirement, Requires, ValidationError};
pub use prices::{Feed, FeedError, Origin, PriceFeed, Priced, Quote};
pub use provider::ProviderId;
pub use recipe::{from_provider, Inputs, Recipe, RecipeError, RECIPE_VERSION};
pub use registry::{launch_models, registry, Model};
pub use route::{Route, RouteError, RoutePolicy};
pub use use_case::UseCase;
