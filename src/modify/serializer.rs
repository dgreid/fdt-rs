// Copyright (c) 2022 by Rivos Inc.
// Licensed under the Apache License, Version 2.0, see LICENSE for details.
// SPDX-License-Identifier: Apache-2.0

use crate::base::parse::ParsedTok;
use crate::base::DevTree;

use crate::error::{DevTreeError, Result};

use crate::fallible_iterator::FallibleIterator;

use crate::modify::modtoken::{ModifyParsedTok, ModifyTokenResponse};

use crate::spec::FdtTok::*;
use crate::spec::FDT_MAGIC;

use crate::priv_util::{SliceWrite, SliceWriteResult};

use core::mem::size_of;

/// A Serializer for DevTree. Used to modify a device tree and serialize the modification
/// into an output u8 buffer.
#[derive(Default)]
pub struct Serializer {
    offset: usize,
}

impl Serializer {
    /// Modifies the device tree using the filter_map function to serialize it to the output buffer.
    /// The documentation for this function is the same as the one sppecified in DeviceTree::modify.
    pub fn modify(
        &mut self,
        devtree: &DevTree,
        output: &mut [u8],
        filter_map: &mut dyn FnMut(&mut ModifyParsedTok, usize) -> ModifyTokenResponse,
    ) -> Result<usize> {
        self.serialize_header(devtree, output);
        self.serialize_memory_reservation_block(devtree, output);

        let new_structure_block_size =
            match self.serialize_structure_block(devtree, output, filter_map) {
                Err(e) => return Err(e),
                Ok(s) => s,
            };

        // the strings block appears in a dtb after the structure block. the size of the structure
        // block may have changed, so we need to ensure the strings block goes in some non-occupied
        // location. the easiest solution is to just serialize the strings block offset directly after
        // the structure block instead of wherever it was in the old dtb. however this requires us
        // to update the header with the new values of the strings block offset as well as the
        // size of the structure block.
        let strings_block_offset = self.get_offset();

        self.set_structure_block_size(output, new_structure_block_size);
        self.set_strings_block_offset(output, strings_block_offset);
        self.serialize_strings_block(devtree, output, strings_block_offset);

        // the total size of the fdt may have changed, lets update the header to reflect this
        let total_size = self.get_offset();
        self.set_total_size(output, total_size);
        Ok(total_size)
    }

    fn serialize_header(&mut self, devtree: &DevTree, output: &mut [u8]) {
        self.set_offset(0);

        self.serialize_u32(output, FDT_MAGIC).unwrap();
        self.serialize_u32(output, devtree.totalsize() as u32)
            .unwrap();
        self.serialize_u32(output, devtree.off_dt_struct() as u32)
            .unwrap();
        self.serialize_u32(output, devtree.off_dt_strings() as u32)
            .unwrap();
        self.serialize_u32(output, devtree.off_mem_rsvmap() as u32)
            .unwrap();
        self.serialize_u32(output, devtree.version()).unwrap();
        self.serialize_u32(output, devtree.last_comp_version())
            .unwrap();
        self.serialize_u32(output, devtree.boot_cpuid_phys())
            .unwrap();
        self.serialize_u32(output, devtree.size_dt_strings())
            .unwrap();
        self.serialize_u32(output, devtree.size_dt_struct())
            .unwrap();
    }

    fn serialize_memory_reservation_block(&mut self, devtree: &DevTree, output: &mut [u8]) {
        self.set_offset(devtree.off_mem_rsvmap());

        for entity in devtree.reserved_entries() {
            self.serialize_u64(output, u64::from(entity.address))
                .unwrap();
            self.serialize_u64(output, u64::from(entity.size)).unwrap();
        }
    }

    fn serialize_structure_block(
        &mut self,
        devtree: &DevTree,
        output: &mut [u8],
        filter_map: &mut dyn FnMut(&mut ModifyParsedTok, usize) -> ModifyTokenResponse,
    ) -> Result<usize> {
        // this function returns the new size of the structure block
        // so let's keep track of the starting offset, and subtract it
        // from the offset at the end of the function to get our total
        // size.
        let starting_offset = self.get_offset();

        self.set_offset(devtree.off_dt_struct());

        let mut nodes = devtree.parse_iter();
        while let Ok(Some(token)) = nodes.next() {
            // First, we must modify the output buffer to add the current prop.
            // This is because filter_map is allowed to modify the prop buffer.
            // In order for modification to happen properly, the old prop
            // has to already exist in the buffer. so let's serialize the node.

            // we need to reserve the offset of the node. when we call filter_map,
            // the callback may mutate the node, and so we need to save the current
            // offset so we can apply the changes the callback makes.

            let node_offset = self.get_offset();

            // calculated in the match statement. these values are passed into the
            // callback after serialization

            let original_size;

            let mut modifytoken: ModifyParsedTok = {
                match token.clone() {
                    ParsedTok::BeginNode(inner) => {
                        self.serialize_u32(output, BeginNode as u32).unwrap();
                        original_size = inner.name.len();

                        // a name of length 0 still requires a null terminated character.
                        // so if we see no name, serialize a 0.
                        if inner.name.is_empty() {
                            self.serialize_u32(output, 0).unwrap();
                        } else {
                            self.serialize_string(output, inner.name).unwrap();
                        }

                        ModifyParsedTok::BeginNode(inner)
                    }

                    ParsedTok::Prop(inner) => {
                        self.serialize_u32(output, Prop as u32).unwrap();
                        self.serialize_u32(output, inner.prop_buf.len() as u32)
                            .unwrap();
                        self.serialize_u32(output, inner.name_offset as u32)
                            .unwrap();

                        let prop_offset = self.get_offset();
                        original_size = inner.prop_buf.len();

                        self.serialize_slice(output, inner.prop_buf).unwrap();

                        ModifyParsedTok::Prop(inner, &mut output[prop_offset..])
                    }

                    ParsedTok::EndNode => {
                        self.serialize_u32(output, EndNode as u32).unwrap();

                        original_size = 0;
                        ModifyParsedTok::EndNode
                    }

                    ParsedTok::Nop => {
                        self.serialize_u32(output, Nop as u32).unwrap();

                        original_size = 0;
                        ModifyParsedTok::Nop
                    }
                }
            };

            self.align_offset::<u32>();

            let response = filter_map(&mut modifytoken, original_size);

            match response {
                ModifyTokenResponse::Pass => {}
                ModifyTokenResponse::Drop => {
                    self.set_offset(node_offset);
                } // reset the offset to the saved value from earlier

                ModifyTokenResponse::ModifySize(new_size) => {
                    // update the prop size based on the result of filtermap

                    if let ParsedTok::Prop(inner) = token {
                        self.set_offset(node_offset + 4); // + 4 to skip the token header

                        self.serialize_u32(output, new_size as u32).unwrap();
                        self.serialize_u32(output, inner.name_offset as u32)
                            .unwrap();

                        self.set_offset(self.get_offset() + new_size);
                    } else {
                        return Err(DevTreeError::InvalidParameter(
                            "Cannot return ModifySize from a non-Prop token!",
                        ));
                    }
                }
            }

            self.align_offset::<u32>();
        }

        self.serialize_u32(output, End as u32).unwrap();

        Ok(self.get_offset() - starting_offset)
    }

    fn set_structure_block_size(&mut self, output: &mut [u8], structure_block_size: usize) {
        self.set_offset(36);
        self.serialize_u32(output, structure_block_size as u32)
            .unwrap();
    }

    fn set_strings_block_offset(&mut self, output: &mut [u8], strings_block_offset: usize) {
        self.set_offset(12);
        self.serialize_u32(output, strings_block_offset as u32)
            .unwrap();
    }

    fn set_total_size(&mut self, output: &mut [u8], total_size: usize) {
        self.set_offset(4);
        self.serialize_u32(output, total_size as u32).unwrap();
    }

    fn serialize_strings_block(&mut self, devtree: &DevTree, output: &mut [u8], offset: usize) {
        self.set_offset(offset);

        self.serialize_slice(
            output,
            &devtree.buf()[devtree.off_dt_strings()
                ..devtree.off_dt_strings() + devtree.size_dt_strings() as usize],
        )
        .unwrap();

        self.align_offset::<u32>();
    }

    fn align_offset<T>(&mut self) {
        let misalignment = self.offset % size_of::<T>();
        if misalignment != 0 {
            self.offset += size_of::<T>() - misalignment;
        }
    }

    fn serialize_u32(&mut self, buf: &mut [u8], val: u32) -> SliceWriteResult {
        let result = buf.write_be_u32(self.offset, val);
        self.offset += 4;

        result
    }

    fn serialize_u64(&mut self, buf: &mut [u8], val: u64) -> SliceWriteResult {
        let result = buf.write_be_u64(self.offset, val);
        self.offset += 8;

        result
    }

    fn serialize_slice(&mut self, buf: &mut [u8], val: &[u8]) -> SliceWriteResult {
        let result = buf.write_slice(self.offset, val);
        self.offset += val.len();

        result
    }

    fn serialize_string(&mut self, buf: &mut [u8], val: &[u8]) -> SliceWriteResult {
        let result = buf.write_bstring0(self.offset, val);
        self.offset += val.len() + 1;

        result
    }

    fn set_offset(&mut self, new_offset: usize) {
        self.offset = new_offset;
    }

    fn get_offset(&self) -> usize {
        self.offset
    }
}
