use clap::{ArgMatches, Command, Arg};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use crate::blockchain::proto::block::Block;
use crate::blockchain::proto::script::ScriptPattern;
use crate::callbacks::Callback;
use crate::common::Result;

pub struct OpReturn {
    writer: BufWriter<File>,
}

impl Callback for OpReturn {
    fn build_subcommand() -> Command
    where
        Self: Sized,
    {
        Command::new("opreturn")
            .about("Exports OP_RETURN trend data to CSV")
            .version("0.1")
            .author("gcarq <egger.m@protonmail.com>")
            .arg(
                Arg::new("output")
                    .help("Output CSV file path")
                    .value_name("FILE")
                    .required(true)
                    .index(1),
            )
    }

    fn new(matches: &ArgMatches) -> Result<Self>
    where
        Self: Sized,
    {
        let output_path = PathBuf::from(matches.get_one::<String>("output").unwrap());
        let file = File::create(output_path)?;
        // Use smaller 4KB buffer to reduce RAM usage
        let mut writer = BufWriter::with_capacity(4096, file);
        
        // Write CSV header
        writeln!(
            writer,
            "block_height,block_timestamp,txid,data_length,tx_output_index"
        )?;
        
        Ok(OpReturn { writer })
    }

    fn on_start(&mut self, _: u64) -> Result<()> {
        info!(target: "callback", "Executing OpReturn ...");
        Ok(())
    }

    fn on_block(&mut self, block: &Block, block_height: u64) -> Result<()> {
        let block_timestamp = block.header.value.timestamp;
        
        for tx in &block.txs {
            for (output_index, out) in tx.value.outputs.iter().enumerate() {
                if let ScriptPattern::OpReturn(data) = &out.script.pattern {
                    if data.is_empty() {
                        continue;
                    }
                    
                    // Write CSV row
                    writeln!(
                        self.writer,
                        "{},{},{},{},{}",
                        block_height,
                        block_timestamp,
                        &tx.hash,
                        data.len(),
                        output_index
                    )?;
                }
            }
        }
        Ok(())
    }

    fn on_complete(&mut self, _: u64) -> Result<()> {
        self.writer.flush()?;
        Ok(())
    }

    fn show_progress(&self) -> bool {
        false
    }
}