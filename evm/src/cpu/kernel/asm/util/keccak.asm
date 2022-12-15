global keccak_general_be:
    // stack: addr: 3, len, retdest
    KECCAK_GENERAL
    %reverse_bytes_u256
    // stack: hash_be, retdest
    SWAP1
    JUMP

%macro keccak_general_be
    %stack (addr: 3, len) -> (addr, len, %%after)
    %jump(keccak_general_be)
%%after:
%endmacro

%macro keccak_general_le
    KECCAK_GENERAL
%endmacro

global sys_keccak256:
    // stack: kexit_info, offset, len
    %stack (kexit_info, offset, len) -> (offset, len, kexit_info)
    PUSH @SEGMENT_MAIN_MEMORY
    GET_CONTEXT
    // stack: ADDR: 3, len, kexit_info
    %keccak_general_be
    // stack: hash, kexit_info
    SWAP1
    EXIT_KERNEL

// Computes Keccak256(input_word). Clobbers @SEGMENT_KERNEL_GENERAL.
//
// Pre stack: input_word
// Post stack: hash
%macro keccak256_word(num_bytes)
    // Since KECCAK_GENERAL takes its input from memory, we will first write
    // input_word's bytes to @SEGMENT_KERNEL_GENERAL[0..$num_bytes].
    %stack (word) -> (0, @SEGMENT_KERNEL_GENERAL, 0, word, $num_bytes, %%after_mstore)
    %jump(mstore_unpacking)
%%after_mstore:
    // stack: offset
    %stack (offset) -> (0, @SEGMENT_KERNEL_GENERAL, 0, $num_bytes) // context, segment, offset, len
    KECCAK_GENERAL
%endmacro
