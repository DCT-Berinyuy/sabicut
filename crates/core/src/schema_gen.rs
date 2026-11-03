//! JSON Schema generation shared by the file formats.

use schemars::JsonSchema;

/// The JSON Schema (draft 2020-12) of `T`, pretty-printed with a trailing newline.
pub(crate) fn pretty<T: JsonSchema>() -> String {
    let generator = schemars::generate::SchemaSettings::draft2020_12().into_generator();
    format!("{:#}\n", generator.into_root_schema_for::<T>().as_value())
}
