// Copyright (c) 2022, Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use anyhow::anyhow;
use base64ct::{Base64, Encoding};
use move_core_types::language_storage::TypeTag;
use move_core_types::parser::parse_type_tag;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_with::serde_as;

use sui_types::base_types::{ObjectDigest, ObjectID, ObjectRef, SequenceNumber, TransactionDigest};
use sui_types::error::SuiError;
use sui_types::object::{ObjectRead, Owner, SuiMoveData};

#[serde_as]
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ObjectResponse {
    pub objects: Vec<NamedObjectRef>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NamedObjectRef {
    /// Hex code as string representing the object id
    object_id: String,
    /// Object version.
    version: u64,
    /// Base64 string representing the object digest
    digest: String,
}

impl NamedObjectRef {
    pub fn to_object_ref(self) -> Result<ObjectRef, anyhow::Error> {
        Ok((
            ObjectID::try_from(self.object_id)?,
            SequenceNumber::from(self.version),
            ObjectDigest::try_from(&*Base64::decode_vec(&self.digest).map_err(|e| anyhow!(e))?)?,
        ))
    }
}

impl From<ObjectRef> for NamedObjectRef {
    fn from((object_id, version, digest): ObjectRef) -> Self {
        Self {
            object_id: format!("{:#x}", object_id),
            version: version.value(),
            digest: Base64::encode_string(digest.as_ref()),
        }
    }
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ObjectExistsResponse {
    object_ref: NamedObjectRef,
    owner: Owner,
    previous_transaction: TransactionDigest,
    data: SuiMoveData,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ObjectNotExistsResponse {
    object_id: String,
}

#[allow(clippy::large_enum_variant)]
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", content = "details")]
pub enum GetObjectInfoResponse {
    Exists(ObjectExistsResponse),
    NotExists(ObjectNotExistsResponse),
    Deleted(NamedObjectRef),
}

impl TryFrom<ObjectRead> for GetObjectInfoResponse {
    type Error = SuiError;

    fn try_from(obj: ObjectRead) -> Result<Self, Self::Error> {
        match obj {
            ObjectRead::Exists(object_ref, object, layout) => {
                Ok(Self::Exists(ObjectExistsResponse {
                    object_ref: object_ref.into(),
                    owner: object.owner,
                    previous_transaction: object.previous_transaction,
                    data: object.data.to_json(&layout)?,
                }))
            }
            ObjectRead::NotExists(object_id) => Ok(Self::NotExists(ObjectNotExistsResponse {
                object_id: object_id.to_hex(),
            })),
            ObjectRead::Deleted(obj_ref) => Ok(Self::Deleted(obj_ref.into())),
        }
    }
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename = "TypeTagString")]
pub struct SuiTypeTag(String);

impl TryInto<TypeTag> for SuiTypeTag {
    type Error = anyhow::Error;
    fn try_into(self) -> Result<TypeTag, Self::Error> {
        parse_type_tag(&self.0)
    }
}

impl From<TypeTag> for SuiTypeTag {
    fn from(tag: TypeTag) -> Self {
        Self(format!("{}", tag))
    }
}
