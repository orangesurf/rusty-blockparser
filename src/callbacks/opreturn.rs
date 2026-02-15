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
        let mut writer = BufWriter::with_capacity(4096, file);
        
        // Write CSV header
        writeln!(
            writer,
            "block_height,block_timestamp,txid,tx_output_index,is_push_only,is_one_data_push,data_length,total_bytes_after_return"
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
                if let ScriptPattern::OpReturn(_) = &out.script.pattern {
                    let script_bytes = &out.out.script_pubkey;
                    
                    let (is_push_only, is_one_data_push, data_length, total_bytes) = 
                        analyze_op_return_script(script_bytes);
                    
                    let data_length_str = match data_length {
                        Some(len) => len.to_string(),
                        None => String::new(),
                    };
                    
                    writeln!(
                        self.writer,
                        "{},{},{},{},{},{},{},{}",
                        block_height,
                        block_timestamp,
                        &tx.hash,
                        output_index,
                        is_push_only,
                        is_one_data_push,
                        data_length_str,
                        total_bytes
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

/// Analyzes OP_RETURN script to determine compliance with Bitcoin Core policies
/// Returns: (is_push_only, is_one_data_push, data_length, total_bytes_after_return)
fn analyze_op_return_script(script_bytes: &[u8]) -> (bool, bool, Option<usize>, usize) {
    let op_return_pos = script_bytes.iter().position(|&b| b == 0x6a);
    if op_return_pos.is_none() {
        return (false, false, None, 0);
    }
    
    let after_return = &script_bytes[op_return_pos.unwrap() + 1..];
    let total_bytes_after_return = after_return.len();
    
    if after_return.is_empty() {
        return (true, true, Some(0), 0);
    }
    
    // Check if all opcodes are push-only (≤ 0x60)
    let is_push_only = check_push_only(after_return);
    
    // Check if it's a single well-formed data push
    let (is_one_data_push, data_length) = check_one_data_push(after_return);
    
    (is_push_only, is_one_data_push, data_length, total_bytes_after_return)
}

/// Checks if all opcodes in the script are push-only (≤ 0x60)
/// This matches Bitcoin Core's IsPushOnly() requirement
fn check_push_only(script_bytes: &[u8]) -> bool {
    let mut pos = 0;
    
    while pos < script_bytes.len() {
        let opcode = script_bytes[pos];
        
        // Reject any opcode > 0x60 (OP_16)
        if opcode > 0x60 {
            return false;
        }
        
        // Calculate how many bytes to skip
        let skip = match opcode {
            0x00 => 1, // OP_0
            0x01..=0x4b => 1 + opcode as usize, // OP_PUSHBYTES_N
            0x4c => { // OP_PUSHDATA1
                if pos + 1 >= script_bytes.len() {
                    return false;
                }
                2 + script_bytes[pos + 1] as usize
            }
            0x4d => { // OP_PUSHDATA2
                if pos + 2 >= script_bytes.len() {
                    return false;
                }
                let len = u16::from_le_bytes([script_bytes[pos + 1], script_bytes[pos + 2]]) as usize;
                3 + len
            }
            0x4e => { // OP_PUSHDATA4
                if pos + 4 >= script_bytes.len() {
                    return false;
                }
                let len = u32::from_le_bytes([
                    script_bytes[pos + 1], script_bytes[pos + 2],
                    script_bytes[pos + 3], script_bytes[pos + 4]
                ]) as usize;
                5 + len
            }
            0x4f..=0x60 => 1, // OP_1NEGATE through OP_16
            _ => return false,
        };
        
        pos += skip;
    }
    
    pos == script_bytes.len()
}

/// Checks if script is exactly one well-formed data push
fn check_one_data_push(script_bytes: &[u8]) -> (bool, Option<usize>) {
    if script_bytes.is_empty() {
        return (false, None);
    }
    
    let first_byte = script_bytes[0];
    
    let (declared_len, data_start_offset) = match first_byte {
        1..=75 => (first_byte as usize, 1),
        0x4c => {
            if script_bytes.len() < 2 {
                return (false, None);
            }
            (script_bytes[1] as usize, 2)
        }
        0x4d => {
            if script_bytes.len() < 3 {
                return (false, None);
            }
            let len = u16::from_le_bytes([script_bytes[1], script_bytes[2]]) as usize;
            (len, 3)
        }
        0x4e => {
            if script_bytes.len() < 5 {
                return (false, None);
            }
            let len = u32::from_le_bytes([
                script_bytes[1], script_bytes[2], script_bytes[3], script_bytes[4]
            ]) as usize;
            (len, 5)
        }
        _ => return (false, None),
    };
    
    let actual_data_available = script_bytes.len() - data_start_offset;
    let is_one_push = declared_len == actual_data_available 
                     && data_start_offset + declared_len == script_bytes.len();
    
    if is_one_push {
        (true, Some(declared_len))
    } else {
        (false, None)
    }
}