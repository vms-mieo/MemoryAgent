use serde::Deserialize;
use serde::Serialize;
use uuid::Uuid;

fn new_v4() -> Uuid {
    Uuid::new_v4()
}

macro_rules! define_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);
        impl $name {
            pub fn new() -> Self {
                Self(new_v4())
            }
            pub fn from_uuid(id: Uuid) -> Self {
                Self(id)
            }
            pub fn as_uuid(&self) -> &Uuid {
                &self.0
            }
            pub fn to_string_value(&self) -> String {
                self.0.to_string()
            }
        }
        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }
        impl std::str::FromStr for $name {
            type Err = uuid::Error;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self(Uuid::parse_str(s)?))
            }
        }
    };
}

define_id!(ProjectId);
define_id!(SessionId);
define_id!(MemoryId);
define_id!(RevisionId);
define_id!(EntityId);
define_id!(RelationId);
