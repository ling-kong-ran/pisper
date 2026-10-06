//! @larksuiteoapi/node-sdk 1.72.0 的 pbbp2 protobuf 字段契约。
use prost::Message;
#[derive(Clone, PartialEq, Message)]
pub(super) struct Header {
    #[prost(string, required, tag = "1")]
    pub(super) key: String,
    #[prost(string, required, tag = "2")]
    pub(super) value: String,
}
#[derive(Clone, PartialEq, Message)]
pub(super) struct Frame {
    #[prost(uint64, required, tag = "1")]
    pub(super) seq_id: u64,
    #[prost(uint64, required, tag = "2")]
    pub(super) log_id: u64,
    #[prost(int32, required, tag = "3")]
    pub(super) service: i32,
    #[prost(int32, required, tag = "4")]
    pub(super) method: i32,
    #[prost(message, repeated, tag = "5")]
    pub(super) headers: Vec<Header>,
    #[prost(string, optional, tag = "6")]
    pub(super) payload_encoding: Option<String>,
    #[prost(string, optional, tag = "7")]
    pub(super) payload_type: Option<String>,
    #[prost(bytes = "vec", optional, tag = "8")]
    pub(super) payload: Option<Vec<u8>>,
    #[prost(string, optional, tag = "9")]
    pub(super) log_id_new: Option<String>,
}
impl Frame {
    pub(super) fn header(&self, key: &str) -> &str {
        self.headers
            .iter()
            .find(|header| header.key == key)
            .map(|header| header.value.as_str())
            .unwrap_or("")
    }
}
