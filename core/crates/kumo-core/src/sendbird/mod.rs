pub mod frame;
pub mod model;
pub mod rest;
pub mod socket;

pub use frame::Frame;
pub use model::*;
pub use rest::{ChannelListQuery, FileUpload, MessageQuery, SendbirdRest, UploadedFile};
pub use socket::{Connected, FileSend, SbEvent, SendbirdSocket, SocketParams};

pub const SDK_VERSION: &str = "4.22.14";

pub fn user_agent() -> String {
    format!("JS/c{SDK_VERSION}///o{}", std::env::consts::OS)
}

pub fn sdk_user_agent() -> String {
    format!(
        "main_sdk_info=chat/js/{SDK_VERSION}&device_os_platform={}&os_version=",
        std::env::consts::OS
    )
}

pub fn encode_component(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}
