%macro mload_trie_data
    // stack: virtual
    %mload_kernel(@SEGMENT_TRIE_DATA)
    // stack: value
%endmacro

%macro mstore_trie_data
    // stack: virtual, value
    %mstore_kernel(@SEGMENT_TRIE_DATA)
    // stack: (empty)
%endmacro

%macro initialize_rlp_segment
    PUSH @ENCODED_EMPTY_NODE_POS
    PUSH 0x80
    MSTORE_GENERAL
%endmacro

%macro alloc_rlp_block
    // stack: (empty)
    %mload_global_metadata(@GLOBAL_METADATA_RLP_DATA_SIZE)
    // stack: block_start
    // In our model it's fine to use memory in a sparse way, as long as the gaps aren't larger than
    // 2^16 or so. So instead of the caller specifying the size of the block they need, we'll just
    // allocate 0x10000 = 2^16 bytes, much larger than any RLP blob the EVM could possibly create.
    DUP1 %add_const(@MAX_RLP_BLOB_SIZE)
    // stack: block_end, block_start
    %mstore_global_metadata(@GLOBAL_METADATA_RLP_DATA_SIZE)
    // stack: block_start
    // We leave an extra 9 bytes, so that callers can later prepend a prefix before block_start.
    // (9 is the length of the longest possible RLP list prefix.)
    %add_const(9)
    // stack: block_start
%endmacro

%macro get_trie_data_size
    // stack: (empty)
    %mload_global_metadata(@GLOBAL_METADATA_TRIE_DATA_SIZE)
    // stack: trie_data_size
%endmacro

%macro set_trie_data_size
    // stack: trie_data_size
    %mstore_global_metadata(@GLOBAL_METADATA_TRIE_DATA_SIZE)
    // stack: (empty)
%endmacro

// Equivalent to: trie_data[trie_data_size++] = value
%macro append_to_trie_data
    // stack: value
    %get_trie_data_size
    // stack: trie_data_size, value
    DUP1
    %increment
    // stack: trie_data_size', trie_data_size, value
    %set_trie_data_size
    // stack: trie_data_size, value
    %mstore_trie_data
    // stack: (empty)
%endmacro

// Split off the first nibble from a key part. Roughly equivalent to
// def split_first_nibble(num_nibbles, key):
//     num_nibbles -= 1
//     num_nibbles_x4 = num_nibbles * 4
//     first_nibble = (key >> num_nibbles_x4) & 0xF
//     key -= (first_nibble << num_nibbles_x4)
//     return (first_nibble, num_nibbles, key)
%macro split_first_nibble
    // stack: num_nibbles, key
    %decrement // num_nibbles -= 1
    // stack: num_nibbles, key
    DUP2
    // stack: key, num_nibbles, key
    DUP2 %mul_const(4)
    // stack: num_nibbles_x4, key, num_nibbles, key
    SHR
    // stack: key >> num_nibbles_x4, num_nibbles, key
    %and_const(0xF)
    // stack: first_nibble, num_nibbles, key
    DUP1
    // stack: first_nibble, first_nibble, num_nibbles, key
    DUP3 %mul_const(4)
    // stack: num_nibbles_x4, first_nibble, first_nibble, num_nibbles, key
    SHL
    // stack: first_nibble << num_nibbles_x4, first_nibble, num_nibbles, key
    DUP1
    // stack: junk, first_nibble << num_nibbles_x4, first_nibble, num_nibbles, key
    SWAP4
    // stack: key, first_nibble << num_nibbles_x4, first_nibble, num_nibbles, junk
    SUB
    // stack: key, first_nibble, num_nibbles, junk
    SWAP3
    // stack: junk, first_nibble, num_nibbles, key
    POP
    // stack: first_nibble, num_nibbles, key
%endmacro

// Remove the first `k` nibbles from a key part.
// def truncate_nibbles(k, num_nibbles, key):
//     num_nibbles -= k
//     num_nibbles_x4 = num_nibbles * 4
//     lead_nibbles = key >> num_nibbles_x4
//     key -= (lead_nibbles << num_nibbles_x4)
//     return (num_nibbles, key)
%macro truncate_nibbles
    // stack: k, num_nibbles, key
    SWAP1 SUB
    // stack: num_nibbles, key
    DUP1 %mul_const(4)
    %stack (num_nibbles_x4, num_nibbles, key) -> (num_nibbles_x4, key, num_nibbles_x4, num_nibbles, key)
    SHR
    %stack (lead_nibbles, num_nibbles_x4, num_nibbles, key) -> (num_nibbles_x4, lead_nibbles, key, num_nibbles)
    SHL SWAP1 SUB
    // stack: key, num_nibbles
    SWAP1
%endmacro

// Split off the common prefix among two key parts.
//
// Pre stack: len_1, key_1, len_2, key_2
// Post stack: len_common, key_common, len_1, key_1, len_2, key_2
//
// Roughly equivalent to
// def split_common_prefix(len_1, key_1, len_2, key_2):
//     bits_1 = len_1 * 4
//     bits_2 = len_2 * 4
//     len_common = 0
//     key_common = 0
//     while True:
//         if bits_1 * bits_2 == 0:
//             break
//         first_nib_1 = (key_1 >> (bits_1 - 4)) & 0xF
//         first_nib_2 = (key_2 >> (bits_2 - 4)) & 0xF
//         if first_nib_1 != first_nib_2:
//             break
//         len_common += 1
//         key_common = key_common * 16 + first_nib_1
//         bits_1 -= 4
//         bits_2 -= 4
//         key_1 -= (first_nib_1 << bits_1)
//         key_2 -= (first_nib_2 << bits_2)
//     len_1 = bits_1 // 4
//     len_2 = bits_2 // 4
//     return (len_common, key_common, len_1, key_1, len_2, key_2)
%macro split_common_prefix
    // stack: len_1, key_1, len_2, key_2
    %mul_const(4)
    SWAP2 %mul_const(4) SWAP2
    // stack: bits_1, key_1, bits_2, key_2
    PUSH 0
    PUSH 0

%%loop:
    // stack: len_common, key_common, bits_1, key_1, bits_2, key_2

    // if bits_1 * bits_2 == 0: break
    DUP3 DUP6 MUL ISZERO %jumpi(%%return)

    // first_nib_2 = (key_2 >> (bits_2 - 4)) & 0xF
    DUP6 PUSH 4 DUP7 SUB SHR %and_const(0xF)
    // first_nib_1 = (key_1 >> (bits_1 - 4)) & 0xF
    DUP5 PUSH 4 DUP6 SUB SHR %and_const(0xF)
    // stack: first_nib_1, first_nib_2, len_common, key_common, bits_1, key_1, bits_2, key_2

    // if first_nib_1 != first_nib_2: break
    DUP2 DUP2 SUB %jumpi(%%return_with_first_nibs)

    // len_common += 1
    SWAP2 %increment SWAP2

    // key_common = key_common * 16 + first_nib_1
    SWAP3
    %mul_const(16)
    DUP4 ADD
    SWAP3
    // stack: first_nib_1, first_nib_2, len_common, key_common, bits_1, key_1, bits_2, key_2

    // bits_1 -= 4
    SWAP4 %sub_const(4) SWAP4
    // bits_2 -= 4
    SWAP6 %sub_const(4) SWAP6
    // stack: first_nib_1, first_nib_2, len_common, key_common, bits_1, key_1, bits_2, key_2

    // key_1 -= (first_nib_1 << bits_1)
    DUP5 SHL
    // stack: first_nib_1 << bits_1, first_nib_2, len_common, key_common, bits_1, key_1, bits_2, key_2
    DUP6 SUB
    // stack: key_1, first_nib_2, len_common, key_common, bits_1, key_1_old, bits_2, key_2
    SWAP5 POP
    // stack: first_nib_2, len_common, key_common, bits_1, key_1, bits_2, key_2

    // key_2 -= (first_nib_2 << bits_2)
    DUP6 SHL
    // stack: first_nib_2 << bits_2, len_common, key_common, bits_1, key_1, bits_2, key_2
    DUP7 SUB
    // stack: key_2, len_common, key_common, bits_1, key_1, bits_2, key_2_old
    SWAP6 POP
    // stack: len_common, key_common, bits_1, key_1, bits_2, key_2

    %jump(%%loop)
%%return_with_first_nibs:
    // stack: first_nib_1, first_nib_2, len_common, key_common, bits_1, key_1, bits_2, key_2
    %pop2
%%return:
    // stack: len_common, key_common, bits_1, key_1, bits_2, key_2
    SWAP2 %shr_const(2) SWAP2 // bits_1 -> len_1 (in nibbles)
    SWAP4 %shr_const(2) SWAP4 // bits_2 -> len_2 (in nibbles)
    // stack: len_common, key_common, len_1, key_1, len_2, key_2
%endmacro

// Remove the first `k` nibbles from a key part.
// def merge_nibbles(front_len, front_key, back_len, back_key):
//     return (front_len + back_len, (front_key<<(back_len*4)) + back_key)
%macro merge_nibbles
    // stack: front_len, front_key, back_len, back_key
    %stack (front_len, front_key, back_len, back_key) -> (back_len, front_key, back_key, back_len, front_len)
    %mul_const(4) SHL ADD
    // stack: new_key, back_len, front_len
    SWAP2 ADD
%endmacro

// Computes state_key = Keccak256(addr). Clobbers @SEGMENT_KERNEL_GENERAL.
%macro addr_to_state_key
    %keccak256_word(20)
%endmacro

// Given a storage slot (a 256-bit integer), computes storage_key = Keccak256(slot).
// Clobbers @SEGMENT_KERNEL_GENERAL.
%macro slot_to_storage_key
    %keccak256_word(32)
%endmacro
