use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

#[must_use]
/// 使用进程随机源生成按时间排序的 UUID 第 7 版。
pub fn new_id() -> Uuid {
    Uuid::now_v7()
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
/// UUID 的规范 16 字节存储表示。
///
/// 序列化和显示使用带连字符的标准 UUID 字符串；数据库边界可使用
/// [`Self::as_bytes`]，无需重新分配内存。
pub struct IdBlob([u8; 16]);

/// `SQLite` 支撑的 Core 记录共用的标识符类型。
pub type Id = IdBlob;

impl IdBlob {
    #[must_use]
    /// 生成新的 `UUIDv7` 标识符。
    pub fn new() -> Self {
        Self::from(new_id())
    }

    #[must_use]
    /// 借用用于 blob 持久化的准确 16 字节。
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    #[must_use]
    /// 不改变已存储字节地将其转换为 UUID 值。
    pub const fn into_uuid(self) -> Uuid {
        Uuid::from_bytes(self.0)
    }
}

impl Default for IdBlob {
    fn default() -> Self {
        Self::new()
    }
}

impl From<Uuid> for IdBlob {
    fn from(value: Uuid) -> Self {
        Self(*value.as_bytes())
    }
}

impl From<IdBlob> for Uuid {
    fn from(value: IdBlob) -> Self {
        value.into_uuid()
    }
}

impl TryFrom<&[u8]> for IdBlob {
    type Error = uuid::Error;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        Uuid::from_slice(value).map(Self::from)
    }
}

impl fmt::Display for IdBlob {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.into_uuid().fmt(formatter)
    }
}

impl Serialize for IdBlob {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.into_uuid().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for IdBlob {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Uuid::deserialize(deserializer).map(Self::from)
    }
}

#[cfg(test)]
mod tests {
    use super::{IdBlob, new_id};

    #[test]
    fn uuid_v7_round_trips_through_sixteen_byte_blob() {
        let uuid = new_id();
        let blob = IdBlob::from(uuid);

        assert_eq!(uuid.get_version_num(), 7);
        assert_eq!(blob.as_bytes().len(), 16);
        assert_eq!(blob.into_uuid(), uuid);
    }

    #[test]
    fn id_blob_json_is_a_canonical_uuid_string() {
        let uuid = new_id();
        let json = serde_json::to_string(&IdBlob::from(uuid)).expect("serialize ID");
        let decoded: IdBlob = serde_json::from_str(&json).expect("deserialize ID");

        assert_eq!(json, format!("\"{uuid}\""));
        assert_eq!(decoded.into_uuid(), uuid);
    }
}
