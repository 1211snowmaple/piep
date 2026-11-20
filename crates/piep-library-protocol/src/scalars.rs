use piep_storage::{Error, Fingerprint, Result};
use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Id(Uuid);

impl Id {
    pub fn new(value: Uuid) -> Result<Self> {
        if value.is_nil() {
            return Err(Error::Integrity("識別子が空です".into()));
        }
        Ok(Self(value))
    }
    pub fn get(self) -> Uuid {
        self.0
    }
}
impl Serialize for Id {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.hyphenated().to_string())
    }
}
impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        let id = Uuid::parse_str(&value).map_err(D::Error::custom)?;
        if id.is_nil() || id.hyphenated().to_string() != value {
            return Err(D::Error::custom("UUID は空でない小文字の標準形が必要です"));
        }
        Ok(Self(id))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hash(Fingerprint);
impl Hash {
    pub fn new(value: Fingerprint) -> Self {
        Self(value)
    }
    pub fn get(self) -> Fingerprint {
        self.0
    }
}
impl Serialize for Hash {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}
impl<'de> Deserialize<'de> for Hash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value.bytes().any(|b| b.is_ascii_uppercase()) {
            return Err(D::Error::custom("ハッシュは小文字の hex が必要です"));
        }
        Fingerprint::parse(&value)
            .map(Self)
            .map_err(D::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decimal(u64);
impl Decimal {
    pub fn new(value: u64) -> Self {
        Self(value)
    }
    pub fn get(self) -> u64 {
        self.0
    }
}
impl Serialize for Decimal {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}
impl<'de> Deserialize<'de> for Decimal {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value.is_empty()
            || value.len() > 20
            || (value.len() > 1 && value.starts_with('0'))
            || !value.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(D::Error::custom(
                "u64 は符号・小数・余分なゼロのない十進文字列が必要です",
            ));
        }
        value.parse().map(Self).map_err(D::Error::custom)
    }
}
