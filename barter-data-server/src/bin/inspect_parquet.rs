//! Inspect Parquet file metadata + schema (Rust-native).
//!
//! Usage:
//!   cargo run -p barter-data-server --bin inspect_parquet -- /path/to/file.parquet

use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::file::reader::{FileReader, SerializedFileReader};
use std::env;
use std::fs::File;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args().nth(1).ok_or("missing parquet path argument")?;

    let file = File::open(&path)?;
    let reader = SerializedFileReader::new(file)?;
    let metadata = reader.metadata();
    let num_rows = metadata.file_metadata().num_rows();
    let row_groups = metadata.num_row_groups();

    let file = File::open(&path)?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file)?;
    let schema = builder.schema().clone();

    println!("Parquet: {}", path);
    println!("Rows: {}", num_rows);
    println!("Row groups: {}", row_groups);
    println!("Schema fields: {}", schema.fields().len());
    println!("Schema metadata: {:?}", schema.metadata());
    println!("Fields:");
    for field in schema.fields() {
        println!("  - {}: {:?}", field.name(), field.data_type());
    }

    Ok(())
}
