//! Platform services the runtime needs from the host: where it keeps files,
//! how it stores secrets, and how it describes itself to servers.

pub mod capabilities;
pub mod secret_store;

pub use capabilities::{ClientPresentation, default_data_dir};
pub use secret_store::{FileSecretStore, KeychainSecretStore, SecretStore, SecretStoreError, open_secret_store};
