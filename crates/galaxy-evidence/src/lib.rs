//! Capture evidence model: ARKit frame ingest and quantization (depth, confidence, intrinsics, poses, IMU, Bluetooth/noise/meta aggregates), commitment computation via galaxy-crypto, and the `GXEV` EvidenceBundle codec (spec §5, §6).

pub mod bundle;
mod commitments;
pub mod ingest;
mod model;
pub mod synthetic;
mod validation;

pub use commitments::{commit, component_elements, try_commit, Commitments};
pub use galaxy_image::ImageProfile;
pub use model::{
    BluetoothAggregate, CameraFrame, CanonicalImage, Evidence, FrameData, ImuEvidence, ImuSample,
    Meta, NoiseAggregate, Salts, BLUETOOTH_ELEMENTS, CAMERA_ELEMENTS, IMU_ELEMENTS, IMU_MAX,
    META_ELEMENTS, NOISE_ELEMENTS,
};
pub use validation::ValidationError;
