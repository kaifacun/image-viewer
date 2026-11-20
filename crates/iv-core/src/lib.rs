//! 이미지 뷰어 공용 타입과 확장 지점(trait).
//!
//! 이 crate 는 어떤 디코더·렌더러·UI 도 모른다. 새 포맷은 [`Decoder`] 를 구현한
//! 별도 crate 로 추가하고 [`DecoderRegistry`] 에 등록만 하면 된다.

mod cancel;
mod decoder;
mod error;
mod frame;
mod gate;
mod registry;
mod source;

pub use cancel::CancelToken;
pub use decoder::{Caps, Decoder, FrameStream, Priority, SizeHint};
pub use error::{DecodeError, Result};
pub use gate::Gate;
pub use frame::{ColorInfo, Frame, ImageInfo, Orientation, PixelFormat};
pub use registry::DecoderRegistry;
pub use source::ImageSource;
