use clap::{ArgMatches, Command, Arg};
use fxhash::FxHashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use crate::blockchain::proto::block::Block;
use crate::blockchain::proto::tx::TxOutpoint;
use crate::blockchain::proto::ToRaw;
use crate::callbacks::Callback;
use crate::common::Result;

// Minimal struct - only store what we need for fee calculation
#[derive(Clone)]
struct Unspent {
    value: u64,
}

pub struct LowFee {
    writer: BufWriter<File>,
    unspents: FxHashMap<[u8; 36], Unspent>,
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
        let mut writer = BufWriter::with_capacity(1_048_576, file);
        
        writeln!(&mut writer, "block_height,block_timestamp,txid,fee_sats,size_vb,fee_rate")?;
        
        Ok(LowFee {
            writer,
            unspents: FxHashMap::with_capacity_and_hasher(200_000_000, Default::default()),
        })
    }

    fn on_start(&mut self, _: u64) -> Result<()> {
        info!(target: "callback", "Executing LowFee with minimal memory footprint...");
        Ok(())
    }

    fn on_block(&mut self, block: &Block, block_height: u64) -> Result<()> {
        let block_timestamp = block.header.value.timestamp;
        
        for tx in &block.txs {
            // Process coinbase - only add outputs, no inputs to remove
            if tx.value.is_coinbase() {
                for (i, output) in tx.value.outputs.iter().enumerate() {
                    if output.script.address.is_some() {
                        let outpoint = TxOutpoint::new(tx.hash, i as u32);
                        let key = Self::outpoint_to_key(&outpoint);
                        self.unspents.insert(key, Unspent { value: output.out.value });
                    }
                }
                continue;
            }
            
            // Calculate input value by looking up UTXOs
            let mut input_value: u64 = 0;
            for input in &tx.value.inputs {
                let key = Self::outpoint_to_key(&input.outpoint);
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
            let fee_rate = if size_vb > 0.0 {
                fee as f64 / size_vb
            } else {
                0.0
            };
            
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
            
            // Remove spent outputs
            for input in &tx.value.inputs {
                let key = Self::outpoint_to_key(&input.outpoint);
                self.unspents.remove(&key);
            }
            
            // Add new outputs
            for (i, output) in tx.value.outputs.iter().enumerate() {
                if output.script.address.is_some() {
                    let outpoint = TxOutpoint::new(tx.hash, i as u32);
                    let key = Self::outpoint_to_key(&outpoint);
                    self.unspents.insert(key, Unspent { value: output.out.value });
                }
            }
        }
        Ok(())
    }

    fn on_complete(&mut self, _: u64) -> Result<()> {
        self.writer.flush()?;
        info!(target: "callback", "Peak UTXO count: {}", self.unspents.len());
        Ok(())
    }

    fn show_progress(&self) -> bool {
        true
    }
}

impl LowFee {
    // Convert TxOutpoint to fixed-size key
    #[inline]
    fn outpoint_to_key(outpoint: &TxOutpoint) -> [u8; 36] {
        let bytes = outpoint.to_bytes();
        let mut key = [0u8; 36];
        key.copy_from_slice(&bytes);
        key
    }
}