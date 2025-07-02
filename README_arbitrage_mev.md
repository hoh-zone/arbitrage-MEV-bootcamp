# arbitrage MEV lession

## Supports

- BlueMove
- FlowX
- Aftermath
- Cetus 
- Kriya
- Abex
- Navi
- Turbos
- Deepbook
- Shio


## Run 
Start the bot with your private key.

```bash
cargo run -r --bin arb start-bot -- --private-key {}
```

## Relay
If you have a validator, you can let the validator push mempool transactions to your relay, which then send to the bot.

```bash
cargo run -r --bin relay
```