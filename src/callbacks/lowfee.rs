use clap::{ArgMatches, Command, Arg};
use fxhash::FxHashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use crate::blockchain::proto::block::Block;
use crate::blockchain::proto::ToRaw;
use crate::callbacks::{Callback, common};
use crate::common::Result;

pub struct LowFee {
    writer: BufWriter<File>,
    unspents: FxHashMap<[u8; 36], common::UnspentValue>,
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
        // Larger buffer for better I/O performance
        let mut writer = BufWriter::with_capacity(1_048_576, file);
        
        writeln!(&mut writer, "block_height,block_timestamp,txid,fee_sats,size_vb,fee_rate")?;
        
        Ok(LowFee {
            writer,
            // Pre-allocate for peak UTXO count to avoid reallocations
            unspents: FxHashMap::with_capacity_and_hasher(200_000_000, Default::default()),
        })
    }

    fn on_start(&mut self, _: u64) -> Result<()> {
        info!(target: "callback", "Executing LowFee with optimized memory usage...");
        Ok(())
    }

    fn on_block(&mut self, block: &Block, block_height: u64) -> Result<()> {
        let block_timestamp = block.header.value.timestamp;
        
        for tx in &block.txs {
            if tx.value.is_coinbase() {
                self.insert_unspents(tx, block_height);
                continue;
            }
            
            // Calculate input value by looking up UTXOs
            let mut input_value: u64 = 0;
            for input in &tx.value.inputs {
                let key = self.outpoint_to_key(&input.outpoint);
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
            self.remove_unspents(tx);
            self.insert_unspents(tx, block_height);
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
    // Convert TxOutpoint to fixed-size key (saves 24 bytes per entry vs Vec<u8>)
    #[inline]
    fn outpoint_to_key(&self, outpoint: &crate::blockchain::proto::tx::TxOutpoint) -> [u8; 36] {
        let bytes = outpoint.to_bytes();
        let mut key = [0u8; 36];
        key.copy_from_slice(&bytes);
        key
    }
    
    fn remove_unspents(&mut self, tx: &crate::blockchain::proto::Hashed<crate::blockchain::proto::tx::EvaluatedTx>) {
        for input in &tx.value.inputs {
            let key = self.outpoint_to_key(&input.outpoint);
            self.unspents.remove(&key);
        }
    }
    
    fn insert_unspents(&mut self, tx: &crate::blockchain::proto::Hashed<crate::blockchain::proto::tx::EvaluatedTx>, block_height: u64) {
        for (i, output) in tx.value.outputs.iter().enumerate() {
            if let Some(address) = &output.script.address {
                let unspent = common::UnspentValue {
                    block_height,
                    value: output.out.value,
                    address: address.clone(),
                };
                let outpoint = crate::blockchain::proto::tx::TxOutpoint::new(tx.hash, i as u32);
                let key = self.outpoint_to_key(&outpoint);
                self.unspents.insert(key, unspent);
            }
        }
    }
}