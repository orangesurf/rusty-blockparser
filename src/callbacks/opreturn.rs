use clap::{ArgMatches, Command, Arg};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use crate::blockchain::proto::block::Block;
use crate::blockchain::proto::script::ScriptPattern;
use crate::blockchain::proto::ToRaw;
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
            "block_height,block_timestamp,txid,tx_output_index,is_push_only,is_one_data_push,data_length,total_bytes_after_return,prefix_hex,is_coinbase,tx_weight"
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
            let is_coinbase = tx.value.is_coinbase();
            // Check if this tx has any OP_RETURN outputs before computing weight
            let has_opreturn = tx.value.outputs.iter().any(|o| {
                matches!(&o.script.pattern, ScriptPattern::OpReturn(_))
            });
            let tx_weight = if has_opreturn {
                let base_size = tx.value.to_bytes().len() as u64;
                base_size * 4 + tx.value.witness_size as u64
            } else {
                0
            };

            for (output_index, out) in tx.value.outputs.iter().enumerate() {
                if let ScriptPattern::OpReturn(_) = &out.script.pattern {
                    let script_bytes = &out.out.script_pubkey;

                    let (is_push_only, is_one_data_push, data_length, total_bytes) =
                        analyze_op_return_script(script_bytes);

                    let data_length_str = match data_length {
                        Some(len) => len.to_string(),
                        None => String::new(),
                    };

                    let prefix_hex = extract_data_prefix_hex(script_bytes);

                    writeln!(
                        self.writer,
                        "{},{},{},{},{},{},{},{},{},{},{}",
                        block_height,
                        block_timestamp,
                        &tx.hash,
                        output_index,
                        is_push_only,
                        is_one_data_push,
                        data_length_str,
                        total_bytes,
                        prefix_hex,
                        is_coinbase,
                        tx_weight
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

/// Extract the first 20 bytes of pushed DATA from the first push after OP_RETURN.
/// Returns hex-encoded string (up to 40 hex chars). Strips push opcodes.
/// Special case: OP_13 (0x5d) is treated as a 1-byte Runes protocol tag,
/// followed by data from subsequent pushes (up to 19 more bytes).
fn extract_data_prefix_hex(script_bytes: &[u8]) -> String {
    let op_return_pos = match script_bytes.iter().position(|&b| b == 0x6a) {
        Some(p) => p,
        None => return String::new(),
    };
    let after = &script_bytes[op_return_pos + 1..];
    if after.is_empty() {
        return String::new();
    }

    let mut data_bytes: Vec<u8> = Vec::with_capacity(20);

    // OP_13 (0x5d) = Runes protocol tag: include the byte, then extract from subsequent pushes
    if after[0] == 0x5d {
        data_bytes.push(0x5d);
        let mut pos = 1;
        while pos < after.len() && data_bytes.len() < 20 {
            let opcode = after[pos];
            match opcode {
                0x01..=0x4b => {
                    let len = opcode as usize;
                    let start = pos + 1;
                    let end = std::cmp::min(start + len, after.len());
                    let take = std::cmp::min(end - start, 20 - data_bytes.len());
                    data_bytes.extend_from_slice(&after[start..start + take]);
                    pos = start + len;
                }
                0x4c => {
                    if pos + 1 >= after.len() { break; }
                    let len = after[pos + 1] as usize;
                    let start = pos + 2;
                    let end = std::cmp::min(start + len, after.len());
                    let take = std::cmp::min(end - start, 20 - data_bytes.len());
                    data_bytes.extend_from_slice(&after[start..start + take]);
                    pos = start + len;
                }
                0x4d => {
                    if pos + 2 >= after.len() { break; }
                    let len = u16::from_le_bytes([after[pos + 1], after[pos + 2]]) as usize;
                    let start = pos + 3;
                    let end = std::cmp::min(start + len, after.len());
                    let take = std::cmp::min(end - start, 20 - data_bytes.len());
                    data_bytes.extend_from_slice(&after[start..start + take]);
                    pos = start + len;
                }
                0x4e => {
                    if pos + 4 >= after.len() { break; }
                    let len = u32::from_le_bytes([
                        after[pos + 1], after[pos + 2], after[pos + 3], after[pos + 4]
                    ]) as usize;
                    let start = pos + 5;
                    let end = std::cmp::min(start + len, after.len());
                    let take = std::cmp::min(end - start, 20 - data_bytes.len());
                    data_bytes.extend_from_slice(&after[start..start + take]);
                    pos = start + len;
                }
                0x00 | 0x4f..=0x60 => {
                    // OP_0 or number opcodes — skip, no data
                    pos += 1;
                }
                _ => break, // non-push opcode
            }
        }
    } else {
        // Standard first-push extraction
        let first = after[0];
        let (data_start, data_len) = match first {
            0x01..=0x4b => (1usize, first as usize),
            0x4c => {
                if after.len() < 2 { return String::new(); }
                (2, after[1] as usize)
            }
            0x4d => {
                if after.len() < 3 { return String::new(); }
                (3, u16::from_le_bytes([after[1], after[2]]) as usize)
            }
            0x4e => {
                if after.len() < 5 { return String::new(); }
                (5, u32::from_le_bytes([after[1], after[2], after[3], after[4]]) as usize)
            }
            _ => return String::new(), // number opcodes or other — no extractable data
        };
        let available = after.len() - data_start;
        let actual_len = std::cmp::min(data_len, available);
        let take = std::cmp::min(actual_len, 20);
        data_bytes.extend_from_slice(&after[data_start..data_start + take]);
    }

    if data_bytes.is_empty() {
        return String::new();
    }
    data_bytes.iter().map(|b| format!("{:02x}", b)).collect()
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