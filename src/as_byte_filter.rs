use byte_unit::{Byte, UnitType};
use std::collections::HashMap;
use tera::{try_get_value, Error, Filter, Number, Value};

pub struct AsBytesFilter;

impl Filter for AsBytesFilter {
    fn filter(&self, value: &Value, _args: &HashMap<String, Value>) -> Result<Value, Error> {
        let v = try_get_value!("as_bytes", "value", Number, value);
        let byte = Byte::from_u64(v.as_u64().unwrap_or(0));
        let adjusted_byte = byte.get_appropriate_unit(UnitType::Binary);
        Ok(Value::String(format!("{adjusted_byte:.2}")))
    }
}
