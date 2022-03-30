// Copyright (c) 2022 by Rivos Inc.
// Licensed under the Apache License, Version 2.0, see LICENSE for details.
// SPDX-License-Identifier: Apache-2.0

use crate::base::parse::*;

/// The modify callback will return a ModifyTokenResponse that tells the Serializer
/// what operation to perform on the token
pub enum ModifyTokenResponse {
    /// Perform no modification to the token
    Pass,
    /// Remove the token from the device tree
    Drop,
    /// Change the token's size
    ModifySize(usize),
}

/// The Serializer will pass in a ModifyParsedTok that the callback can operate on.
pub enum ModifyParsedTok<'a> {
    /// A begin node marks the beginning of a node. It contains a ParsedBeginNode
    /// object. This object can be passed or dropped, but its size cannot be changed.
    BeginNode(ParsedBeginNode<'a>),
    /// Marks the end of a node. Again, its size cannot be changed.
    EndNode,
    /// Marks a property within a node. The passed in buffer is the property data.
    /// You can write as much data as you need to this buffer. If the size changes,
    /// make sure to return ModifySize to tell the Serializer to resize the dt to
    /// fit your modifications.
    Prop(ParsedProp<'a>, &'a mut [u8]),
    /// Marks a nop within the dt.
    Nop,
}
