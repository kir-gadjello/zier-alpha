pub mod approval;
pub mod bus;
pub mod controller;
pub mod debounce;
pub mod handler;
pub mod telegram_client;
pub mod types;

pub use approval::{ApprovalCoordinator, ApprovalDecision, ApprovalUIRequest};
pub use bus::{IngressBus, IngressProvider};
pub use debounce::{DebounceManager, DebounceSession};
pub use handler::process_ingress_message;
pub use telegram_client::{
    RealTelegramClient, TelegramApi, TelegramAudio, TelegramCallbackQuery, TelegramClient,
    TelegramDocument, TelegramMessage, TelegramPhotoSize, TelegramUpdate, TelegramUser,
    TelegramVoice,
};
pub use types::{IngressMessage, TrustLevel};
