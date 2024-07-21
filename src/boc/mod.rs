//! BOC (Bag Of Cells) implementation.

use crate::boc::ser::PreHashedHasher;
use crate::cell::{Cell, CellBuilder, CellContext, CellFamily, DynCell, HashBytes, Load, Store};

/// BOC decoder implementation.
pub mod de;
/// BOC encoder implementation.
pub mod ser;

/// BOC file magic number.
#[derive(Default, Copy, Clone, Eq, PartialEq)]
pub enum BocTag {
    /// Single root, cells index, no CRC32.
    Indexed,
    /// Single root, cells index, with CRC32.
    IndexedCrc32,
    /// Multiple roots, optional cells index, optional CRC32.
    #[default]
    Generic,
}

impl BocTag {
    const INDEXED: [u8; 4] = [0x68, 0xff, 0x65, 0xf3];
    const INDEXED_CRC32: [u8; 4] = [0xac, 0xc3, 0xa7, 0x28];
    const GENERIC: [u8; 4] = [0xb5, 0xee, 0x9c, 0x72];

    /// Tries to match bytes with BOC tag.
    pub const fn from_bytes(data: [u8; 4]) -> Option<Self> {
        match data {
            Self::GENERIC => Some(Self::Generic),
            Self::INDEXED_CRC32 => Some(Self::IndexedCrc32),
            Self::INDEXED => Some(Self::Indexed),
            _ => None,
        }
    }

    /// Converts BOC tag to bytes.
    pub const fn to_bytes(self) -> [u8; 4] {
        match self {
            Self::Indexed => Self::INDEXED,
            Self::IndexedCrc32 => Self::INDEXED_CRC32,
            Self::Generic => Self::GENERIC,
        }
    }
}

/// A serde helper to use [`Boc`] inside [`Option`].
#[cfg(feature = "serde")]
pub struct OptionBoc;

#[cfg(feature = "serde")]
impl OptionBoc {
    /// Serializes an optional cell into an encoded BOC
    /// (as base64 for human readable serializers).
    pub fn serialize<S, T>(cell: &Option<T>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
        T: AsRef<DynCell>,
    {
        match cell {
            Some(cell) => serializer.serialize_some(cell.as_ref()),
            None => serializer.serialize_none(),
        }
    }

    /// Deserializes an optional cell from an encoded BOC
    /// (from base64 for human readable deserializers).
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Cell>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::Deserialize;

        #[derive(Deserialize)]
        #[repr(transparent)]
        struct Wrapper(#[serde(with = "Boc")] Cell);

        Ok(ok!(Option::<Wrapper>::deserialize(deserializer)).map(|Wrapper(cell)| cell))
    }
}

/// BOC (Bag Of Cells) helper.
pub struct Boc;

impl Boc {
    /// Computes a simple SHA256 hash of the data.
    #[inline]
    pub fn file_hash(data: impl AsRef<[u8]>) -> HashBytes {
        use sha2::Digest;

        sha2::Sha256::digest(data).into()
    }

    /// Encodes the specified cell tree as BOC and
    /// returns the `base64` encoded bytes as a string.
    #[cfg(any(feature = "base64", test))]
    pub fn encode_base64<T>(cell: T) -> String
    where
        T: AsRef<DynCell>,
    {
        crate::util::encode_base64(Self::encode(cell))
    }

    /// Encodes the specified cell tree as BOC.
    pub fn encode<T>(cell: T) -> Vec<u8>
    where
        T: AsRef<DynCell>,
    {
        fn encode_impl(cell: &DynCell) -> Vec<u8> {
            let mut result = Vec::new();
            ser::BocHeader::new(cell, ahash::RandomState::new()).encode(&mut result);
            result
        }
        encode_impl(cell.as_ref())
    }

    /// Encodes a pair of cell trees as BOC.
    pub fn encode_pair<T1, T2>((cell1, cell2): (T1, T2)) -> Vec<u8>
    where
        T1: AsRef<DynCell>,
        T2: AsRef<DynCell>,
    {
        fn encode_pair_impl(cell1: &DynCell, cell2: &DynCell) -> Vec<u8> {
            let mut result = Vec::new();
            let mut encoder = ser::BocHeader::new(cell1, ahash::RandomState::new());
            encoder.add_root(cell2);
            encoder.encode(&mut result);
            result
        }
        encode_pair_impl(cell1.as_ref(), cell2.as_ref())
    }

    /// Decodes a `base64` encoded BOC into a cell tree
    /// using an empty cell context.
    #[cfg(any(feature = "base64", test))]
    #[inline]
    pub fn decode_base64<T: AsRef<[u8]>>(data: T) -> Result<Cell, de::Error> {
        fn decode_base64_impl(data: &[u8]) -> Result<Cell, de::Error> {
            match crate::util::decode_base64(data) {
                Ok(data) => Boc::decode_ext(data.as_slice(), &mut Cell::empty_context()),
                Err(_) => Err(de::Error::UnknownBocTag),
            }
        }
        decode_base64_impl(data.as_ref())
    }

    /// Decodes a cell tree using an empty cell context.
    #[inline]
    pub fn decode<T>(data: T) -> Result<Cell, de::Error>
    where
        T: AsRef<[u8]>,
    {
        fn decode_impl(data: &[u8]) -> Result<Cell, de::Error> {
            Boc::decode_ext(data, &mut Cell::empty_context())
        }
        decode_impl(data.as_ref())
    }

    /// Decodes a pair of cell trees using an empty cell context.
    #[inline]
    pub fn decode_pair<T>(data: T) -> Result<(Cell, Cell), de::Error>
    where
        T: AsRef<[u8]>,
    {
        fn decode_pair_impl(data: &[u8]) -> Result<(Cell, Cell), de::Error> {
            Boc::decode_pair_ext(data, &mut Cell::empty_context())
        }
        decode_pair_impl(data.as_ref())
    }

    /// Decodes a cell tree using the specified cell context.
    pub fn decode_ext(data: &[u8], context: &mut dyn CellContext) -> Result<Cell, de::Error> {
        use self::de::*;

        let header = ok!(de::BocHeader::decode(
            data,
            &Options {
                max_roots: Some(1),
                min_roots: Some(1),
            },
        ));

        if let Some(&root) = header.roots().first() {
            let cells = ok!(header.finalize(context));
            if let Some(root) = cells.get(root) {
                return Ok(root);
            }
        }

        Err(de::Error::RootCellNotFound)
    }

    /// Decodes a pair of cell trees using the specified cell context.
    pub fn decode_pair_ext(
        data: &[u8],
        context: &mut dyn CellContext,
    ) -> Result<(Cell, Cell), de::Error> {
        use self::de::*;

        let header = ok!(de::BocHeader::decode(
            data,
            &Options {
                max_roots: Some(2),
                min_roots: Some(2),
            },
        ));

        let mut roots = header.roots().iter();
        if let (Some(&root1), Some(&root2)) = (roots.next(), roots.next()) {
            let cells = ok!(header.finalize(context));
            if let (Some(root1), Some(root2)) = (cells.get(root1), cells.get(root2)) {
                return Ok((root1, root2));
            }
        }

        Err(de::Error::RootCellNotFound)
    }

    /// Serializes cell into an encoded BOC (as base64 for human readable serializers).
    #[cfg(feature = "serde")]
    pub fn serialize<S, T>(cell: &T, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
        T: AsRef<DynCell> + ?Sized,
    {
        use serde::Serialize;

        cell.as_ref().serialize(serializer)
    }

    /// Deserializes cell from an encoded BOC (from base64 for human readable deserializers).
    #[cfg(feature = "serde")]
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Cell, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;

        let is_human_readable = deserializer.is_human_readable();
        let mut boc = ok!(borrow_cow_bytes(deserializer));

        if is_human_readable {
            match crate::util::decode_base64(boc) {
                Ok(bytes) => {
                    boc = std::borrow::Cow::Owned(bytes);
                }
                Err(_) => return Err(Error::custom("invalid base64 string")),
            }
        }

        match Boc::decode(boc) {
            Ok(cell) => Ok(cell),
            Err(e) => Err(Error::custom(e)),
        }
    }
}

/// BOC representation helper.
pub struct BocRepr;

impl BocRepr {
    /// Encodes the specified cell tree as BOC using an empty cell context and
    /// returns the `base64` encoded bytes as a string.
    #[cfg(any(feature = "base64", test))]
    pub fn encode_base64<T>(data: T) -> Result<String, crate::error::Error>
    where
        T: Store,
    {
        let boc = ok!(Self::encode_ext(data, &mut Cell::empty_context()));
        Ok(crate::util::encode_base64(boc))
    }

    /// Encodes the specified cell tree as BOC using an empty cell context.
    pub fn encode<T>(data: T) -> Result<Vec<u8>, crate::error::Error>
    where
        T: Store,
    {
        Self::encode_ext(data, &mut Cell::empty_context())
    }

    /// Decodes a `base64` encoded BOC into an object
    /// using an empty cell context.
    #[cfg(any(feature = "base64", test))]
    #[inline]
    pub fn decode_base64<T, D>(data: D) -> Result<T, BocReprError>
    where
        for<'a> T: Load<'a>,
        D: AsRef<[u8]>,
    {
        fn decode_base64_impl<T>(data: &[u8]) -> Result<T, BocReprError>
        where
            for<'a> T: Load<'a>,
        {
            match crate::util::decode_base64(data) {
                Ok(data) => BocRepr::decode_ext(data.as_slice(), &mut Cell::empty_context()),
                Err(_) => Err(BocReprError::InvalidBoc(de::Error::UnknownBocTag)),
            }
        }
        decode_base64_impl::<T>(data.as_ref())
    }

    /// Decodes an object using an empty cell context.
    #[inline]
    pub fn decode<T, D>(data: D) -> Result<T, BocReprError>
    where
        for<'a> T: Load<'a>,
        D: AsRef<[u8]>,
    {
        fn decode_impl<T>(data: &[u8]) -> Result<T, BocReprError>
        where
            for<'a> T: Load<'a>,
        {
            BocRepr::decode_ext(data, &mut Cell::empty_context())
        }
        decode_impl::<T>(data.as_ref())
    }
}

impl BocRepr {
    /// Encodes the specified object as BOC.
    pub fn encode_ext<T>(
        data: T,
        context: &mut dyn CellContext,
    ) -> Result<Vec<u8>, crate::error::Error>
    where
        T: Store,
    {
        fn encode_ext_impl(
            data: &dyn Store,
            context: &mut dyn CellContext,
        ) -> Result<Vec<u8>, crate::error::Error> {
            let mut builder = CellBuilder::new();
            ok!(data.store_into(&mut builder, context));
            let cell = ok!(builder.build_ext(context));
            Ok(Boc::encode(cell))
        }
        encode_ext_impl(&data, context)
    }

    /// Decodes object from BOC using the specified cell context.
    pub fn decode_ext<T>(data: &[u8], context: &mut dyn CellContext) -> Result<T, BocReprError>
    where
        for<'a> T: Load<'a>,
    {
        let cell = match Boc::decode_ext(data, context) {
            Ok(cell) => cell,
            Err(e) => return Err(BocReprError::InvalidBoc(e)),
        };

        match cell.as_ref().parse::<T>() {
            Ok(data) => Ok(data),
            Err(e) => Err(BocReprError::InvalidData(e)),
        }
    }

    /// Serializes the type into an encoded BOC using an empty cell context
    /// (as base64 for human readable serializers).
    #[cfg(feature = "serde")]
    pub fn serialize<S, T>(data: &T, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
        T: Store,
    {
        use serde::ser::{Error, Serialize};

        let context = &mut Cell::empty_context();

        let mut builder = CellBuilder::new();
        if data.store_into(&mut builder, context).is_err() {
            return Err(Error::custom("cell overflow"));
        }

        let cell = match builder.build_ext(context) {
            Ok(cell) => cell,
            Err(_) => return Err(Error::custom("failed to store into builder")),
        };

        cell.as_ref().serialize(serializer)
    }

    /// Deserializes the type from an encoded BOC using an empty cell context
    /// (from base64 for human readable serializers).
    #[cfg(feature = "serde")]
    pub fn deserialize<'de, D, T>(deserializer: D) -> Result<T, D::Error>
    where
        D: serde::Deserializer<'de>,
        for<'a> T: Load<'a>,
    {
        use serde::de::Error;

        let cell = ok!(Boc::deserialize(deserializer));
        match cell.as_ref().parse::<T>() {
            Ok(data) => Ok(data),
            Err(_) => Err(Error::custom("failed to decode object from cells")),
        }
    }
}

/// Error type for BOC repr decoding related errors.
#[derive(Debug, thiserror::Error)]
pub enum BocReprError {
    /// Failed to decode BOC.
    #[error("invalid BOC")]
    InvalidBoc(#[source] de::Error),
    /// Failed to decode data from cells.
    #[error("failed to decode object from cells")]
    InvalidData(#[source] crate::error::Error),
}

#[cfg(feature = "serde")]
fn borrow_cow_bytes<'de: 'a, 'a, D>(deserializer: D) -> Result<std::borrow::Cow<'a, [u8]>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use std::borrow::Cow;

    use serde::de::{Error, Visitor};

    struct CowBytesVisitor;

    impl<'a> Visitor<'a> for CowBytesVisitor {
        type Value = Cow<'a, [u8]>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a byte array")
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: Error,
        {
            Ok(Cow::Owned(v.as_bytes().to_vec()))
        }

        fn visit_borrowed_str<E>(self, v: &'a str) -> Result<Self::Value, E>
        where
            E: Error,
        {
            Ok(Cow::Borrowed(v.as_bytes()))
        }

        fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
        where
            E: Error,
        {
            Ok(Cow::Owned(v.into_bytes()))
        }

        fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
        where
            E: Error,
        {
            Ok(Cow::Owned(v.to_vec()))
        }

        fn visit_borrowed_bytes<E>(self, v: &'a [u8]) -> Result<Self::Value, E>
        where
            E: Error,
        {
            Ok(Cow::Borrowed(v))
        }

        fn visit_byte_buf<E>(self, v: Vec<u8>) -> Result<Self::Value, E>
        where
            E: Error,
        {
            Ok(Cow::Owned(v))
        }
    }

    deserializer.deserialize_bytes(CowBytesVisitor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::decode_base64;

    #[test]
    fn boc_with_crc() {
        let boc_without_crc = decode_base64("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").unwrap();

        let cell = Boc::decode(&boc_without_crc).unwrap();

        let mut boc_with_crc = Vec::new();
        ser::BocHeader::<ahash::RandomState>::new(cell.as_ref(), ahash::RandomState::new())
            .with_crc(true)
            .encode(&mut boc_with_crc);
        assert_eq!(boc_without_crc.len() + 4, boc_with_crc.len());

        let decoded = Boc::decode(&boc_with_crc).unwrap();
        assert_eq!(decoded.as_ref(), cell.as_ref());

        let last_byte = boc_with_crc.last_mut().unwrap();
        *last_byte = !*last_byte;

        assert!(matches!(
            Boc::decode(&boc_with_crc),
            Err(de::Error::InvalidChecksum)
        ));
    }

    #[cfg(feature = "serde")]
    #[allow(unused)]
    #[derive(::serde::Serialize)]
    struct SerdeWithCellRef<'a> {
        cell: &'a DynCell,
    }

    #[cfg(feature = "serde")]
    #[derive(::serde::Serialize, ::serde::Deserialize)]
    struct SerdeWithHashBytes {
        some_hash: crate::cell::HashBytes,
    }

    #[cfg(feature = "serde")]
    #[derive(::serde::Serialize, ::serde::Deserialize)]
    struct SerdeWithCellContainer {
        #[serde(with = "Boc")]
        some_cell: Cell,
    }

    #[cfg(feature = "serde")]
    #[derive(::serde::Serialize, ::serde::Deserialize)]
    struct SerdeWithRepr {
        #[serde(with = "BocRepr")]
        dict: crate::dict::RawDict<32>,
        #[serde(with = "BocRepr")]
        merkle_proof: crate::merkle::MerkleProof,
        #[serde(with = "BocRepr")]
        merkle_update: crate::merkle::MerkleUpdate,
    }

    #[cfg(feature = "serde")]
    #[test]
    fn hex_bytes() {
        let hash: crate::cell::HashBytes = rand::random();

        let test = format!(r#"{{"some_hash":"{hash}"}}"#);
        let SerdeWithHashBytes { some_hash } = serde_json::from_str(&test).unwrap();
        assert_eq!(some_hash, hash);

        let serialized = serde_json::to_string(&SerdeWithHashBytes { some_hash }).unwrap();
        assert_eq!(serialized, test);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn struct_with_cell() {
        let boc = "te6ccgEBAQEAWwAAsUgBUkKKaORs1v/d2CpkdS1rueLjL5EbgaivG/SlIBcUZ5cAKkhRTRyNmt/7uwVMjqWtdzxcZfIjcDUV436UpALijPLQ7msoAAYUWGAAAD6o4PtmhMeK8nJA";

        let test = format!(r#"{{"some_cell":"{boc}"}}"#);
        let SerdeWithCellContainer { some_cell } = serde_json::from_str(&test).unwrap();

        let original = Boc::decode_base64(boc).unwrap();
        assert_eq!(some_cell.as_ref(), original.as_ref());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn struct_with_repr() {
        let boc_dict =
            "te6ccgEBCAEAMAABAcABAgPPQAUCAgEgBAMACQAAADqgAAkAAABQYAIBIAcGAAkAAAAe4AAJAAAAbCA=";
        let boc_dict_escaped =
            "te6ccgEBC\\u0041EAMAABAcABAgPPQAUCAgEgBAMACQAAADqgAAkAAABQYAIBIAcGAAkAAAAe4AAJAAAAbCA=";

        let boc_merkle_proof = "te6ccgECBQEAARwACUYDcijLZ4hNbjcLQiThSx8fvxTaVufKbXsXRYbyiUZApXoADQEiccAJ2Y4sgpswmr6/odN0WmKosRtoIzobXRBE9uCeOA1nuXKSo06DG3E/cAAAdbacX3gRQHLHOx0TQAQCAdURYfZ8pYDdK5k1lnsEEJ4OmIYB/AiU4UX3zVZTToFyVwAAAYRmS/s2iLD7PlLAbpXMmss9gghPB0xDAP4ESnCi++arKadAuSuAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAACAsAMARaACLD7PlLAbpXMmss9gghPB0xDAP4ESnCi++arKadAuSuAQKEgBAYDWxHxKJVQ8mzl7cXFvP64eLF0kcXTFLiwZvYlkQrEFAAw=";
        let boc_merkle_update = "te6ccgECEAEAARwACooEmiQq0C+sMHHtQMrhM1KQs0bAR0to7UTxJ/BQaQGQ83mYWpNZrI3tjuzPRZkP0y+odW6SpuxZc6qHEJbPhzX/oAAFAAUIASEBwAIiA85AAwoiASAEDCIBIAUOAgEgBwYACQAAAAKgAAkAAAAAYCEBwAkiA85ACwooSAEBGK24YcgkheIaweTweCPOdGONsG1894aroQWmpQQGjHEAASIBIA0MKEgBAcoZQygrtOJrqvmwmN7NXJy91VsFFfgo/bXAJjbPwI+zAAIiASAPDihIAQGIedrQvLIQIcZHiObah2QWYzPcsgz02CKj0RfEEjv9NwABKEgBAf96V360Wpctur/NPJVfI6Mc5W43dmQzVmLGk0RxKb5RAAE=";

        let test = format!(
            r#"{{"dict":"{boc_dict_escaped}","merkle_proof":"{boc_merkle_proof}","merkle_update":"{boc_merkle_update}"}}"#
        );
        let SerdeWithRepr {
            dict,
            merkle_proof,
            merkle_update,
        } = serde_json::from_str(&test).unwrap();

        let boc = Boc::decode_base64(boc_dict).unwrap();
        let orig_dict = boc.parse::<crate::dict::RawDict<32>>().unwrap();
        assert_eq!(dict, orig_dict);

        let boc = Boc::decode_base64(boc_merkle_proof).unwrap();
        let orig_merkle_proof = boc.parse::<crate::merkle::MerkleProof>().unwrap();
        assert_eq!(merkle_proof, orig_merkle_proof);

        let boc = Boc::decode_base64(boc_merkle_update).unwrap();
        let orig_merkle_update = boc.parse::<crate::merkle::MerkleUpdate>().unwrap();
        assert_eq!(merkle_update, orig_merkle_update);
    }
}
