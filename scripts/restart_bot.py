#!/usr/bin/env python3

import subprocess
import time
import logging
from datetime import datetime

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s - %(levelname)s - %(message)s",
    handlers=[logging.FileHandler("bot_restarter.log"), logging.StreamHandler()],
)


def restart_bot():
    try:
        # Kill existing session if it exists
        subprocess.run(
            ["tmux", "kill-session", "-t", "mev-arb-bot"], stderr=subprocess.PIPE
        )
        logging.info("Killed existing tmux session `mev-arb-bot`")

        # Create new session
        subprocess.run(["tmux", "new-session", "-d", "-s", "mev-arb-bot"], check=True)
        logging.info("Created new tmux session `mev-arb-bot`")

        # Send the command
        cmd = (
            "ENABLE_RECORD_POOL_RELATED_ID=1 cargo run -r --bin arb start-bot "
            "--private-key {} "
            "--use-db-simulator --max-recent-arbs 5 --workers 10 --num-simulators 18 "
            "--preload-path /home/ubuntu/sui/pool_related_ids.txt "
        )

        subprocess.run(
            ["tmux", "send-keys", "-t", "mev-arb-bot", cmd, "Enter"], check=True
        )
        logging.info("Started bot successfully")

    except subprocess.CalledProcessError as e:
        logging.error(f"Failed to execute tmux command: {e}")
    except Exception as e:
        logging.error(f"Unexpected error occurred: {e}")


def main():
    logging.info("Bot restarter script started")
    interval = 3 * 60 * 60  # 3 hours in seconds

    while True:
        try:
            restart_bot()
            next_restart = datetime.now().timestamp() + interval
            logging.info(
                f"Next restart scheduled at: {datetime.fromtimestamp(next_restart)}"
            )
            time.sleep(interval)
        except KeyboardInterrupt:
            logging.info("Script terminated by user")
            break
        except Exception as e:
            logging.error(f"Error in main loop: {e}")
            time.sleep(60)  # Wait a minute before retrying if there's an error


if __name__ == "__main__":
    main()
