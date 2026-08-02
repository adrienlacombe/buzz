// Live check: the RPC Chain must say true for a known contract and false for an
// address that has never been deployed, using the same code path production uses.
#[tokio::main]
async fn main() {
    use buzz_paymaster::rpc::JsonRpcChain;
    use buzz_paymaster::Chain;
    use starknet_core::types::Felt;
    let url = "https://mainnet.nodes.starknet.org/rpc/v0_10";
    let chain = JsonRpcChain::new(url).unwrap();

    let udc = Felt::from_hex_unchecked(
        "0x041a78e741e5af2fec34b695679bc6891742439f7afb8484ecd7766661ad02bf",
    );
    println!(
        "UDC (deployed)        -> {:?}",
        chain.is_deployed(udc).await
    );

    // The address a real NostrAccount would land at, derived from the freshly
    // declared class and BIP-340 test vector 0. Never deployed.
    let cls = Felt::from_hex_unchecked(
        "0x0414f62ea1ed35f8c7bd3b794d94efc95e01bccf04e0f47211fc198f7f56f537",
    );
    let pk = "f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9";
    let addr = buzz_core::starknet_account::account_address(cls, pk).unwrap();
    println!(
        "derived NostrAccount  -> {:?}   ({addr:#x})",
        chain.is_deployed(addr).await
    );
}
