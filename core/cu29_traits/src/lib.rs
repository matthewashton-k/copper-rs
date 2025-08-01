use bincode::de::{BorrowDecoder, Decoder};
use bincode::enc::Encoder;
use bincode::error::{DecodeError, EncodeError};
use bincode::{BorrowDecode, Decode as dDecode, Decode, Encode, Encode as dEncode};
use compact_str::CompactString;
use cu29_clock::{PartialCuTimeRange, Tov};
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};

/// Common copper Error type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CuError {
    message: String,
    cause: Option<String>,
}

impl Display for CuError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let context_str = match &self.cause {
            Some(c) => c.to_string(),
            None => "None".to_string(),
        };
        write!(f, "{}\n   context:{}", self.message, context_str)?;
        Ok(())
    }
}

impl Error for CuError {}

impl From<&str> for CuError {
    fn from(s: &str) -> CuError {
        CuError {
            message: s.to_string(),
            cause: None,
        }
    }
}

impl From<String> for CuError {
    fn from(s: String) -> CuError {
        CuError {
            message: s,
            cause: None,
        }
    }
}

impl CuError {
    pub fn new_with_cause(message: &str, cause: impl Error) -> CuError {
        CuError {
            message: message.to_string(),
            cause: Some(cause.to_string()),
        }
    }

    pub fn add_cause(mut self, context: &str) -> CuError {
        self.cause = Some(context.into());
        self
    }
}

// Generic Result type for copper.
pub type CuResult<T> = Result<T, CuError>;

/// Defines a basic write, append only stream trait to be able to log or send serializable objects.
pub trait WriteStream<E: Encode>: Sync + Send + Debug {
    fn log(&mut self, obj: &E) -> CuResult<()>;
    fn flush(&mut self) -> CuResult<()> {
        Ok(())
    }
}

/// Defines the types of what can be logged in the unified logger.
#[derive(dEncode, dDecode, Copy, Clone, Debug, PartialEq)]
pub enum UnifiedLogType {
    Empty,             // Dummy default used as a debug marker
    StructuredLogLine, // This is for the structured logs (ie. debug! etc..)
    CopperList,        // This is the actual data log storing activities between tasks.
    FrozenTasks,       // Log of all frozen state of the tasks.
    LastEntry,         // This is a special entry that is used to signal the end of the log.
}
/// Represent the minimum set of traits to be usable as Metadata in Copper.
pub trait Metadata: Default + Debug + Clone + Encode + Decode<()> + Serialize {}

impl Metadata for () {}

/// Key metadata piece attached to every message in Copper.
pub trait CuMsgMetadataTrait {
    /// The time range used for the processing of this message
    fn process_time(&self) -> PartialCuTimeRange;

    /// Small status text for user UI to get the realtime state of task (max 24 chrs)
    fn status_txt(&self) -> &CuCompactString;

    /// The ID of the task that generated this message
    fn task_id(&self) -> u16;
}

/// A generic trait to expose the generated CuStampedDataSet from the task graph.
pub trait ErasedCuStampedData {
    fn payload(&self) -> Option<&dyn erased_serde::Serialize>;
    fn tov(&self) -> Tov;
    fn metadata(&self) -> &dyn CuMsgMetadataTrait;
    fn clear_payload(&mut self);
}

/// Trait to get a vector of type-erased CuStampedDataSet
/// This is used for generic serialization of the copperlists
pub trait ErasedCuStampedDataSet {
    fn cumsgs(&self) -> Vec<&dyn ErasedCuStampedData>;
    fn cumsgs_mut(&mut self) -> Vec<&mut dyn ErasedCuStampedData>;
}

/// Trait to trace back from the CopperList the origin of the messages
pub trait MatchingTasks {
    fn get_all_task_ids() -> &'static [&'static str];
}

/// A CopperListTuple needs to be encodable, decodable and fixed size in memory.
pub trait CopperListTuple:
    bincode::Encode
    + bincode::Decode<()>
    + Debug
    + Serialize
    + ErasedCuStampedDataSet
    + MatchingTasks
    + Default
{
} // Decode forces Sized already

// Also anything that follows this contract can be a payload (blanket implementation)
impl<T> CopperListTuple for T where
    T: bincode::Encode
        + bincode::Decode<()>
        + Debug
        + Serialize
        + ErasedCuStampedDataSet
        + MatchingTasks
        + Default
{
}

// We use this type to convey very small status messages.
// MAX_SIZE from their repr module is not accessible so we need to copy paste their definition for 24
// which is the maximum size for inline allocation (no heap)
pub const COMPACT_STRING_CAPACITY: usize = size_of::<String>();

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CuCompactString(pub CompactString);

impl Encode for CuCompactString {
    fn encode<E: Encoder>(&self, encoder: &mut E) -> Result<(), EncodeError> {
        let CuCompactString(ref compact_string) = self;
        let bytes = compact_string.as_bytes();
        let filtered_len = bytes.iter().filter(|&&b| b != 0).count();
        filtered_len.encode(encoder)?;
        Ok(())
    }
}

impl Debug for CuCompactString {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if self.0.is_empty() {
            return write!(f, "CuCompactString(Empty)");
        }
        write!(f, "CuCompactString({})", self.0)
    }
}

impl<Context> Decode<Context> for CuCompactString {
    fn decode<D: Decoder>(decoder: &mut D) -> Result<Self, DecodeError> {
        Ok(CuCompactString(CompactString::new("")))
    }
}

impl<'de, Context> BorrowDecode<'de, Context> for CuCompactString {
    fn borrow_decode<D: BorrowDecoder<'de>>(decoder: &mut D) -> Result<Self, DecodeError> {
        CuCompactString::decode(decoder)
    }
}

#[cfg(test)]
mod tests {
    use crate::CuCompactString;
    use bincode::{config, decode_from_slice, encode_to_vec};
    use compact_str::CompactString;

    #[test]
    fn test_cucompactstr_encode_decode_empty() {
        let cstr = CuCompactString(CompactString::from(""));
        let config = config::standard();
        let encoded = encode_to_vec(&cstr, config).expect("Encoding failed");
        assert_eq!(encoded.len(), 1); // This encodes the usize 0 in variable encoding so 1 byte which is 0.
        let (decoded, _): (CuCompactString, usize) =
            decode_from_slice(&encoded, config).expect("Decoding failed");
        assert_eq!(cstr.0, decoded.0);
    }

    #[test]
    fn test_cucompactstr_encode_decode_small() {
        let cstr = CuCompactString(CompactString::from("test"));
        let config = config::standard();
        let encoded = encode_to_vec(&cstr, config).expect("Encoding failed");
        assert_eq!(encoded.len(), 5); // This encodes a 4-byte string "test" plus 1 byte for the length prefix.
        let (decoded, _): (CuCompactString, usize) =
            decode_from_slice(&encoded, config).expect("Decoding failed");
        assert_eq!(cstr.0, decoded.0);
    }
}
