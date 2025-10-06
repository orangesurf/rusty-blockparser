use clap::{ArgMatches, Command, Arg};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use crate::blockchain::proto::block::Block;
use crate::blockchain::proto::ToRaw;
use crate::callbacks::{Callback, common};
use crate::common::Result;

pub struct LowFee {
    writer: BufWriter<File>,
    unspents: HashMap<Vec<u8>, common::UnspentValue>,
}

impl Callback for LowFee {
    fn build_subcommand() -> Command
    where
        Self: Sized,
    {
        Command::new("lowfee")
            .about("Exports sub-1 sat/vB transactions to CSV")
            .version("0.1")
            .author("orangesurf")
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
        let mut writer = BufWriter::with_capacity(4096, file);
        
        writeln!(writer, "block_height,block_timestamp,txid,fee_sats,size_vb,fee_rate")?;
        
        Ok(LowFee {
            writer,
            unspents: HashMap::with_capacity(10000000),
        })
    }

    fn on_start(&mut self, _: u64) -> Result<()> {
        info!(target: "callback", "Executing LowFee ...");
        Ok(())
    }

    fn on_block(&mut self, block: &Block, block_height: u64) -> Result<()> {
        let block_timestamp = block.header.value.timestamp;
        
        for tx in &block.txs {
            // Skip coinbase transactions (they don't have fees in the traditional sense)
            if tx.value.is_coinbase() {
                common::insert_unspents(tx, block_height, &mut self.unspents);
                continue;
            }
            
            // Calculate input value by looking up UTXOs
            let mut input_value: u64 = 0;
            for input in &tx.value.inputs {
                let key = input.outpoint.to_bytes();
                if let Some(unspent) = self.unspents.get(&key) {
                    input_value += unspent.value;
                }
            }
            
            // Calculate output value
            let output_value: u64 = tx.value.outputs.iter()
                .map(|o| o.out.value)
                .sum();
            
            // Calculate fee
            let fee = input_value.saturating_sub(output_value);
            
            // Calculate size in virtual bytes
            let size_vb = tx.value.vsize();
            
            // Calculate fee rate
            let fee_rate = fee as f64 / size_vb;
            
            // Write if sub-1 sat/vB
            if fee_rate < 1.0 {
                writeln!(
                    self.writer,
                    "{},{},{},{},{:.2},{:.4}",
                    block_height,
                    block_timestamp,
                    &tx.hash,
                    fee,
                    size_vb,
                    fee_rate
                )?;
            }
            
            // Update UTXO set
            common::remove_unspents(tx, &mut self.unspents);
            common::insert_unspents(tx, block_height, &mut self.unspents);
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