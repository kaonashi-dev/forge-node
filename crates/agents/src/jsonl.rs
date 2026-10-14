use std::borrow::Cow;
use std::io::{BufRead, Read};

use serde::de::{Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use serde_json::Value;

/// What [`for_each`] did not hand over.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Unread {
    /// Records longer than the cap, skipped without being held in memory.
    pub oversized: u32,
    /// A read failed and the rest of the stream was abandoned.
    pub failed: bool,
}

impl Unread {
    #[must_use]
    pub(crate) fn any(self) -> bool {
        self.oversized > 0 || self.failed
    }
}

/// Hand `visit` each newline-delimited record, terminator stripped the way
/// `BufRead::lines` strips it, through one reused buffer.
///
/// A record over `max_bytes` is skipped whole: the buffer never holds more than
/// `max_bytes + 1` bytes of it.
pub(crate) fn for_each(
    mut reader: impl BufRead,
    max_bytes: u64,
    mut visit: impl FnMut(&[u8]),
) -> Unread {
    let mut unread = Unread::default();
    let mut record = Vec::new();
    loop {
        record.clear();
        match reader
            .by_ref()
            .take(max_bytes.saturating_add(1))
            .read_until(b'\n', &mut record)
        {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => {
                unread.failed = true;
                break;
            }
        }
        let terminated = record.last() == Some(&b'\n');
        if (record.len() - usize::from(terminated)) as u64 > max_bytes {
            unread.oversized = unread.oversized.saturating_add(1);
            if !terminated && reader.skip_until(b'\n').is_err() {
                unread.failed = true;
                break;
            }
            continue;
        }
        if terminated {
            record.pop();
            if record.last() == Some(&b'\r') {
                record.pop();
            }
        }
        visit(&record);
    }
    unread
}

/// A string field read the way `Value::as_str` reads one: any other JSON type
/// is consumed and yields `None` instead of failing the record.
#[derive(Default)]
pub(crate) struct Text<'a>(pub(crate) Option<Cow<'a, str>>);

impl Text<'_> {
    pub(crate) fn as_str(&self) -> Option<&str> {
        self.0.as_deref()
    }
}

impl<'de: 'a, 'a> Deserialize<'de> for Text<'a> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct AnyValue;

        impl<'de> Visitor<'de> for AnyValue {
            type Value = Option<Cow<'de, str>>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("any JSON value")
            }

            fn visit_borrowed_str<E>(self, value: &'de str) -> Result<Self::Value, E> {
                Ok(Some(Cow::Borrowed(value)))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
                Ok(Some(Cow::Owned(value.to_owned())))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
                Ok(Some(Cow::Owned(value)))
            }

            fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E> {
                Ok(None)
            }

            fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E> {
                Ok(None)
            }

            fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E> {
                Ok(None)
            }

            fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E> {
                Ok(None)
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(None)
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(None)
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(None)
            }
        }

        deserializer.deserialize_any(AnyValue).map(Text)
    }
}

/// A nested object read the way `Value::get` reads one: anything that is not
/// an object has no fields. A derived struct would otherwise read an array
/// positionally.
pub(crate) struct Object<T>(pub(crate) Option<T>);

impl<T> Default for Object<T> {
    fn default() -> Self {
        Self(None)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Object<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct OnlyMaps<T>(std::marker::PhantomData<T>);

        impl<'de, T: Deserialize<'de>> Visitor<'de> for OnlyMaps<T> {
            type Value = Option<T>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("any JSON value")
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                T::deserialize(serde::de::value::MapAccessDeserializer::new(map)).map(Some)
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(None)
            }

            fn visit_str<E>(self, _: &str) -> Result<Self::Value, E> {
                Ok(None)
            }

            fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E> {
                Ok(None)
            }

            fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E> {
                Ok(None)
            }

            fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E> {
                Ok(None)
            }

            fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E> {
                Ok(None)
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(None)
            }
        }

        deserializer
            .deserialize_any(OnlyMaps(std::marker::PhantomData))
            .map(Object)
    }
}

/// `value[key]` as a string, owned, the way [`Text`] would have read it.
pub(crate) fn text_of(value: &Value, key: &str) -> Text<'static> {
    Text(
        value
            .get(key)
            .and_then(Value::as_str)
            .map(|text| Cow::Owned(text.to_owned())),
    )
}

/// A field that is present, even as `null`, is `Some`: `Value::get` tells a
/// `null` from a missing key, and a typed reader standing in for it must too.
pub(crate) fn present<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

/// Whether a field is there at all, `null` included, for a key whose value
/// is never read.
pub(crate) fn exists<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    IgnoredAny::deserialize(deserializer).map(|_| true)
}

/// A boolean field read the way `Value::as_bool` reads one.
pub(crate) fn flag<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<bool>, D::Error> {
    Value::deserialize(deserializer).map(|value| value.as_bool())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(input: &str, max: u64) -> (Vec<String>, Unread) {
        let mut seen = Vec::new();
        let unread = for_each(input.as_bytes(), max, |record| {
            seen.push(String::from_utf8_lossy(record).into_owned());
        });
        (seen, unread)
    }

    #[test]
    fn terminators_are_stripped_like_lines() {
        let (seen, unread) = collect("a\r\nb\n\nc\r", 16);
        assert_eq!(seen, ["a", "b", "", "c\r"]);
        assert_eq!(unread, Unread::default());
    }

    /// The newline does not count against the cap, so a record of exactly
    /// `max` bytes is kept whether or not it is the last one.
    #[test]
    fn an_oversized_record_is_skipped_and_counted_without_losing_the_next() {
        let (seen, unread) = collect("1234\n12345\nok\n12345", 4);
        assert_eq!(seen, ["1234", "ok"]);
        assert_eq!(unread.oversized, 2);
        assert!(!unread.failed);
    }
}
