//! Test-only contracts.
//!
//! Compiled into the package because `snforge` can only declare classes that
//! exist in the build artifacts. Nothing here is part of the account's surface;
//! it exists so `__execute__` can be tested against a real callee rather than a
//! stub, which is how OpenZeppelin's account tests are structured.

#[starknet::interface]
pub trait ISimpleMock<TState> {
    fn set_value(ref self: TState, value: felt252);
    fn get_value(self: @TState) -> felt252;
    fn always_panics(self: @TState);
}

#[starknet::contract]
pub mod SimpleMock {
    use starknet::storage::{StoragePointerReadAccess, StoragePointerWriteAccess};

    #[storage]
    struct Storage {
        value: felt252,
    }

    #[abi(embed_v0)]
    impl SimpleMockImpl of super::ISimpleMock<ContractState> {
        fn set_value(ref self: ContractState, value: felt252) {
            self.value.write(value);
        }

        fn get_value(self: @ContractState) -> felt252 {
            self.value.read()
        }

        /// Used to check that a failing call reverts the whole multicall.
        fn always_panics(self: @ContractState) {
            panic!("mock panic");
        }
    }
}
