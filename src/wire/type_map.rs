//! [`TypeMap`]: generic hashmap-backed [`WireAdapter`] and
//! [`TypeMapDefaults`]: per-source `defaults()` companion trait.

use alloc::string::ToString;
use alloc::sync::Arc;
use hashbrown::HashMap;

use super::adapter::WireAdapter;
use super::decoder::Decoder;
use super::error::DecodeError;
use super::source::WireSource;
use super::wire_type::WireKind;
use crate::encoding::Value;

/// Generic type-to-decoder registry.
///
/// Keyed by [`WireKind`], the source-independent semantic column type
/// without its parameters. Implements [`WireAdapter`] via a single
/// `HashMap::get` per column on the payload's
/// [`WireType::kind`](super::WireType::kind).
pub struct TypeMap<Src: WireSource, S, B> {
    pub(super) entries: HashMap<WireKind, Arc<dyn Decoder<Src, S, B> + Send + Sync>>,
}

impl<Src: WireSource, S, B> TypeMap<Src, S, B> {
    /// Empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Register (or replace) the decoder for `key`. Returns `&mut self`
    /// for chaining.
    pub fn register<D>(&mut self, key: WireKind, decoder: D) -> &mut Self
    where
        D: Decoder<Src, S, B> + Send + Sync + 'static,
    {
        self.entries.insert(key, Arc::new(decoder));
        self
    }

    /// Same as [`register`](Self::register) but consumes `self` for
    /// builder-style chaining: `TypeMap::new().with(k1, d1).with(k2, d2)`.
    #[must_use]
    pub fn with<D>(mut self, key: WireKind, decoder: D) -> Self
    where
        D: Decoder<Src, S, B> + Send + Sync + 'static,
    {
        self.register(key, decoder);
        self
    }

    /// Number of registered entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when the registry has zero entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl<Src, S, B> TypeMap<Src, S, B>
where
    Src: TypeMapDefaults<S, B>,
{
    /// Pre-populated registry with every self-evident mapping the crate
    /// ships for this source, sugared for
    /// `<Src as TypeMapDefaults<S, B>>::defaults()`.
    #[must_use]
    pub fn defaults() -> Self {
        <Src as TypeMapDefaults<S, B>>::defaults()
    }
}

impl<Src: WireSource, S, B> Default for TypeMap<Src, S, B> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Src: WireSource, S, B> WireAdapter<Src, S, B> for TypeMap<Src, S, B> {
    fn decode(&self, payload: Src::Payload<'_>) -> Result<Value<S, B>, DecodeError> {
        let key = Src::wire_type(&payload).kind();
        match self.entries.get(&key) {
            Some(decoder) => decoder.decode_within(payload, self),
            None => Err(DecodeError::NoDecoderForType {
                column: Src::column_name(&payload).to_string(),
            }),
        }
    }
}

/// Per-source companion trait providing a pre-populated
/// [`TypeMap`] with every self-evident mapping the crate ships.
///
/// Call as `TypeMap::<PgWalstream, String, Vec<u8>>::defaults()`.
///
pub trait TypeMapDefaults<S, B>: WireSource + Sized {
    /// Registry pre-populated with every default mapping for this source.
    fn defaults() -> TypeMap<Self, S, B>;
}
