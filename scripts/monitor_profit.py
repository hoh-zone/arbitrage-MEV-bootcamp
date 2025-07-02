import requests
import time
from decimal import Decimal

profit_address = ""
URL = ''
SUI_ARB_BOT_TOKEN = ""
GROUP_SUI_ARB = ""
THREAD_ONCHAIN_LARGE_PROFIT = ""
BALANCE_DIFF_THRESHOLD = 500000000

profit_address_balance = 0


def get_current_balance(profit_address):
    payload = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "suix_getBalance",
        "params": [profit_address]
    }

    try:
        response = requests.post(URL, json=payload)
        response.raise_for_status()
    except requests.exceptions.HTTPError as http_err:
        print(f'HTTP error occurred: {http_err}')  
    except Exception as err:
        print(f'An error occurred: {err}')

    if response.status_code == 200:
        result = response.json()
        if 'error' in result:
            print(f'Error: {result["error"]}')
        elif 'result' in result:
            return result["result"]["totalBalance"]
        else:
            print('Unexpected response format.')
    else:
        print(f'Response status code: {response.status_code}')
        return 0


def monitor_profit(profit_address):

    global profit_address_balance
    current_profit_address_balance = get_current_balance(profit_address)
    
    if int(current_profit_address_balance) - int(profit_address_balance) >= int(BALANCE_DIFF_THRESHOLD):
    
        profit_tx_hash = get_tx(profit_address=profit_address)
        profit_address_balance_decimal = Decimal(profit_address_balance) / Decimal(10**9)
        current_profit_address_balance_decimal = Decimal(current_profit_address_balance) / Decimal(10**9)
        profit_decimal = current_profit_address_balance_decimal - profit_address_balance_decimal

        if profit_tx_hash:
            profit_tx_hash_md = profit_tx_hash.replace('_', '\\_')  
            msg = (
                f'*Monitor Large Profit tx*: [{profit_tx_hash_md}]({profit_tx_hash_md})\n'
                f'*Previous Balance*: `{profit_address_balance_decimal:.9f}`\n'
                f'*Current Balance*: `{current_profit_address_balance_decimal:.9f}`\n'
                f'*Profit*: `{profit_decimal:.9f}`'
            )
            send_telegram_message(SUI_ARB_BOT_TOKEN, GROUP_SUI_ARB, THREAD_ONCHAIN_LARGE_PROFIT, msg)
    profit_address_balance = current_profit_address_balance

def get_tx(profit_address):
    params = [
        {"filter": {"ToAddress": profit_address}, "options": None},
        None, 
        1, 
        True  
    ]

    payload = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "suix_queryTransactionBlocks",
        "params": params
    }

    try:
        response = requests.post(URL, json=payload)
        response.raise_for_status()
    except requests.exceptions.HTTPError as http_err:
        print(f'HTTP error occurred: {http_err}')
    except Exception as err:
        print(f'An error occurred: {err}')

    tx_hash = None
    if response.status_code == 200:
        result = response.json()
        if 'error' in result:
            print(f'Error: {result["error"]}')
        elif 'result' in result:
            tx_hash = result["result"]["data"][0]["digest"]
            tx_hash = f'https://suivision.xyz/txblock/{tx_hash}'
        else:
            print('Unexpected response format.')
    else:
        print(f'Response status code: {response.status_code}')
    return tx_hash

def send_telegram_message(bot_token, chat_id, thread_id, message):
    url = f"https://api.telegram.org/bot{bot_token}/sendMessage"
    payload = {
        "chat_id": chat_id,
        "message_thread_id": thread_id,
        "text": message,
        "parse_mode": "Markdown"
    }
    response = requests.post(url, json=payload)
    return response.json()

if __name__ == '__main__':
    while True:
        profit_address_balance = get_current_balance(profit_address)
        print("profit_address_balance", profit_address_balance)
        monitor_profit(profit_address)
        time.sleep(1)
