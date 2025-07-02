mod shio_collector;
mod shio_conn;
mod shio_executor;
mod shio_rpc_executor;
mod types;

// (objectID, initialSharedVersion)
pub const SHIO_GLOBAL_STATES: [(&str, u64); 32] = [
    (
        "0xc32ce42eac951759666cbc993646b72387ec2708a2917c2c6fb7d21f00108c18",
        72869622,
    ),
    (
        "0x0289acae0edcdf1fe3aedc2e886bc23064d41c359e0179a18152a64d1c1c2b3e",
        327637282,
    ),
    (
        "0x03132160e8c2c45208abf3ccf165e82edcc42fee2e614afe54582f9740a808b8",
        327637282,
    ),
    (
        "0x072ae7307459e535379f422995a0d10132f12a3450298f8cf0cc07bd164f9999",
        327637282,
    ),
    (
        "0x1c1a96a2f4a34ea09ab15b8ff98f4b6b4338ce89f4158eb7d3eb2cd4dcbd6d86",
        327637282,
    ),
    (
        "0x20d76f37ad9f2421a9e6afaf3bb204250b1c2241c50e8a955e86a1a48767d06f",
        327637282,
    ),
    (
        "0x213ed368233cc7480fcb6336e70c5ae7ee106b2317ba02ccb5d0478e45bcc046",
        327637282,
    ),
    (
        "0x22ce1e80937354eba5549fed2937dc6e2b24026d03505bb51a3e4a64aa4142f6",
        327637282,
    ),
    (
        "0x26188cb7ce5ae633279f440f66081cb65cc585e428de18e194f8843e866f799f",
        327637282,
    ),
    (
        "0x38642f01422480128388d3e2948d3dc1b2680f9914077edb6bd3451ae1c5bcf0",
        327637282,
    ),
    (
        "0x3dd85b6424aea1cae9eff6e55456ca783e056226325f1362106eca8b3ed04ca0",
        327637282,
    ),
    (
        "0x42f8adc490542369d9c3b95e9f6eb70b2583102900feb7e103072ed49ba7fc3d",
        327637282,
    ),
    (
        "0x46b8158c82fa6bda7230d31a127d934c7295a0042083b4900f3096e9191f6f3f",
        327637282,
    ),
    (
        "0x6ebac88a8c3f7a4a9fb05ea49d188a1fe8520ae59ee736e0473004d3033512a4",
        327637282,
    ),
    (
        "0x6f55ad6cb40cfc124c11b11c19be0a80237b104acd955e7b52ccb7bf9046fe33",
        327637282,
    ),
    (
        "0x71aafb8bac986e82e5f78846bf3b36c2a82505585625207324140227a27ff279",
        327637282,
    ),
    (
        "0x7fe9b08680d4179de5672f213b863525b21f10604ca161538075e9338d1d2324",
        327637282,
    ),
    (
        "0x81538ef2909a3e0dd3d7f38bcbee191509bae4e8666272938ced295672e2ee8d",
        327637282,
    ),
    (
        "0x828eb6b3354ad68a23dd792313a16a0d888b7ea4fdb884bb22bd569f8e61319e",
        327637282,
    ),
    (
        "0x9705a332b8c1650dd7fe687ef9f9a9638afb51c30c0b34db150d60b920bc07eb",
        327637282,
    ),
    (
        "0x9918f73797a9390e9888b55454f2b31bc01de1a4634acab08f80641c4248e8a5",
        327637282,
    ),
    (
        "0x9cd4c08bdf2e132ec2cc77b0f03be60a94951e046d8e82ed5494f44e609edd2f",
        327637282,
    ),
    (
        "0xac8ce2033571140509788337c8a1f3aa8941a320ecd7047acda310d39cad9e03",
        327637282,
    ),
    (
        "0xbcd4527035265461a9a7b4f1e57c63ea7a6bdf0dc223c66033c218d880f928b1",
        327637282,
    ),
    (
        "0xbfdb691b8cc0b3c3a3b7a654f6682f3e53b164d9ee00b9582cdb4d0a353440a9",
        327637282,
    ),
    (
        "0xc2559d5c52ae04837ddf943a8c2cd53a5a0b512cee615d30d3abe25aa339465e",
        327637282,
    ),
    (
        "0xc56db634d02511e66d7ca1254312b71c60d64dc44bf67ea46b922c52d8aebba6",
        327637282,
    ),
    (
        "0xc84545cbff1b36b874ab2b69d11a3d108f23562e87550588c0bda335b27101e0",
        327637282,
    ),
    (
        "0xcc141659b5885043f9bfcfe470064819ab9ac667953bcedd1000e0652e90ee76",
        327637282,
    ),
    (
        "0xef6bf4952968d25d3e79f7e4db1dc38f2e9d99d61ad38f3829acb4100fe6383a",
        327637282,
    ),
    (
        "0xf2ed8d00ef829de5c4a3c5adf2d6b0f41f7fec005fb9c88e5616b98173b2fd66",
        327637282,
    ),
    (
        "0xfce73f3c32c3f56ddb924a04cabd44dd870b72954bbe7c3d7767c3b8c25c4326",
        327637282,
    ),
];

pub const SHIO_FEED_URL: &str = "wss://rpc.getshio.com/feed";
pub const SHIO_JSON_RPC_URL: &str = "https://rpc.getshio.com";

pub use shio_collector::ShioCollector;
pub use shio_executor::ShioExecutor;
pub use shio_rpc_executor::ShioRPCExecutor;
pub use types::*;

pub async fn new_shio_collector_and_executor(
    keypair: sui_types::crypto::SuiKeyPair,
    shio_feed_url: Option<String>,
    num_retries: Option<u32>,
) -> (ShioCollector, ShioExecutor) {
    let (bid_sender, shio_item_receiver) = shio_conn::new_shio_conn(
        shio_feed_url.unwrap_or(SHIO_FEED_URL.to_string()),
        num_retries.unwrap_or(3),
    )
    .await;

    let executor = ShioExecutor::new(keypair, bid_sender).await;
    let collector = ShioCollector::new(shio_item_receiver);

    (collector, executor)
}
