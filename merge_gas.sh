#!/bin/bash

if [ "$#" -ne 2 ]; then
    echo "Usage: $0 <account_address> <primary_gas_coin_object_id>"
    exit 1
fi

ACCOUNT_ADDRESS=$1
PRIMARY_COIN=$2

if ! command -v sui &> /dev/null; then
    echo "Error: SUI CLI is not installed. Please install it first."
    exit 1
fi

echo "Fetching all gas coins for account: $ACCOUNT_ADDRESS..."
GAS_COINS=$(sui client gas $ACCOUNT_ADDRESS | grep -Eo '0x[0-9a-fA-F]{64}')
echo "Gas coins: $GAS_COINS"

if [ -z "$GAS_COINS" ]; then
    echo "No gas coins found for account: $ACCOUNT_ADDRESS."
    exit 0
fi

GAS_COINS_ARRAY=($GAS_COINS)

if [[ ! " ${GAS_COINS_ARRAY[@]} " =~ " ${PRIMARY_COIN} " ]]; then
    echo "Error: Primary gas coin $PRIMARY_COIN is not found in the account $ACCOUNT_ADDRESS."
    exit 1
fi

for COIN_TO_MERGE in "${GAS_COINS_ARRAY[@]}"; do
    if [ "$COIN_TO_MERGE" == "$PRIMARY_COIN" ]; then
        continue
    fi

    echo "Merging gas coin $COIN_TO_MERGE into $PRIMARY_COIN..."

    sui client merge-coin --primary-coin $PRIMARY_COIN --coin-to-merge $COIN_TO_MERGE

    if [ $? -ne 0 ]; then
        echo "Error: Failed to merge $COIN_TO_MERGE into $PRIMARY_COIN."
        exit 1
    fi
done

echo "All gas coins have been successfully merged into $PRIMARY_COIN."
