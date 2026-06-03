#!/usr/bin/env python3
"""Minimal x402 seller that speaks haima's wire format and settles on-chain.

Returns a 402 with haima's `payment-required` header; on the signed retry it
decodes haima's `payment-signature` (EIP-3009 authorization + r||s||v) and
submits `USDC.transferWithAuthorization(...)` to Base Sepolia via a relayer
EOA (the gasless-payer model EIP-3009 enables), then returns the real
settlement tx hash in haima's `payment-response` header.

The relayer key is read from /tmp/x402-live/relayer.key (a 0600 EOA key you
generate + ETH-fund via the CDP faucet). The file mode is enforced at start.
No secrets are embedded here.

env: RECIPIENT, AMOUNT (atomic USDC, default 100 = 0.0001 USDC), PORT (8402).
"""
import base64
import json
import os
import time
from http.server import BaseHTTPRequestHandler, HTTPServer

from eth_account import Account
from web3 import Web3

RPC = "https://sepolia.base.org"
NETWORK = "eip155:84532"
USDC = Web3.to_checksum_address("0x036CbD53842c5426634e7929541eC2318f3dCF7e")  # base-sepolia FiatTokenV2
RECIPIENT = Web3.to_checksum_address(
    os.environ.get("RECIPIENT", "0x389b6a704d3b34688863def723b3890453b53aee")
)
AMOUNT = int(os.environ.get("AMOUNT", "100"))  # 0.0001 USDC (<= 100 micro-credit auto-approve cap)
PORT = int(os.environ.get("PORT", "8402"))
RELAYER_KEY_FILE = os.environ.get("RELAYER_KEY_FILE", "/tmp/x402-live/relayer.key")

# CodeRabbit (Major): enforce the documented 0600 on the relayer key file.
_st = os.stat(RELAYER_KEY_FILE)
if _st.st_mode & 0o077:
    raise SystemExit(
        f"refusing to start: {RELAYER_KEY_FILE} is group/other-accessible "
        f"(mode {oct(_st.st_mode & 0o777)}); chmod 600 it"
    )

w3 = Web3(Web3.HTTPProvider(RPC))
relayer = Account.from_key(open(RELAYER_KEY_FILE).read())
ABI = [{"inputs": [{"name": "from", "type": "address"}, {"name": "to", "type": "address"},
                   {"name": "value", "type": "uint256"}, {"name": "validAfter", "type": "uint256"},
                   {"name": "validBefore", "type": "uint256"}, {"name": "nonce", "type": "bytes32"},
                   {"name": "v", "type": "uint8"}, {"name": "r", "type": "bytes32"},
                   {"name": "s", "type": "bytes32"}],
        "name": "transferWithAuthorization", "outputs": [], "stateMutability": "nonpayable",
        "type": "function"}]
usdc = w3.eth.contract(address=USDC, abi=ABI)


def log(m: str) -> None:
    with open("/tmp/x402-live/seller.log", "a") as f:
        f.write(m + "\n")
    print(m, flush=True)


def make_402() -> str:
    header = {"schemes": [{"scheme": "exact", "network": NETWORK, "token": USDC,
                           "amount": str(AMOUNT), "recipient": RECIPIENT,
                           "facilitator": "http://localhost/none", "max_timeout_seconds": 600}],
              "version": "v2"}
    return base64.b64encode(json.dumps(header).encode()).decode()


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):  # quiet
        pass

    def do_GET(self):
        sig = self.headers.get("payment-signature")
        if not sig:
            self.send_response(402)
            self.send_header("payment-required", make_402())
            self.end_headers()
            self.wfile.write(b"payment required")
            return
        try:
            ps = json.loads(base64.b64decode(sig))
            auth = ps["authorization"]
            # CodeRabbit (Critical): the signed authorization MUST match the
            # advertised charge — never settle a recipient/amount/network the
            # 402 did not ask for.
            if ps.get("network") != NETWORK:
                raise ValueError(f"network {ps.get('network')} != advertised {NETWORK}")
            if Web3.to_checksum_address(auth["to"]) != RECIPIENT:
                raise ValueError(f"authorization.to {auth['to']} != advertised recipient {RECIPIENT}")
            if int(auth["value"]) != AMOUNT:
                raise ValueError(f"authorization.value {auth['value']} != advertised amount {AMOUNT}")

            pl = ps["payload"]
            pl = pl[2:] if pl.startswith("0x") else pl
            raw = bytes.fromhex(pl)
            r, s, v = raw[0:32], raw[32:64], raw[64]
            nonce = auth["nonce"]
            nonce = bytes.fromhex(nonce[2:] if nonce.startswith("0x") else nonce)
            log(f"SETTLING from={auth['from']} to={auth['to']} value={auth['value']} v={v}")
            tx = usdc.functions.transferWithAuthorization(
                Web3.to_checksum_address(auth["from"]), Web3.to_checksum_address(auth["to"]),
                int(auth["value"]), int(auth["validAfter"]), int(auth["validBefore"]),
                nonce, v, r, s,
            ).build_transaction({
                "from": relayer.address,
                "nonce": w3.eth.get_transaction_count(relayer.address),
                "gas": 250000,
                "gasPrice": max(int(w3.eth.gas_price), w3.to_wei("0.1", "gwei")),
                "chainId": 84532,
            })
            stx = relayer.sign_transaction(tx)
            txh = w3.eth.send_raw_transaction(stx.raw_transaction)
            log(f"BROADCAST tx={txh.hex()}")
            rc = w3.eth.wait_for_transaction_receipt(txh, timeout=120)
            settled = rc.status == 1
            log(f"RECEIPT tx={txh.hex()} status={rc.status} block={rc.blockNumber}")
            txh_hex = txh.hex()
            resp = {"tx_hash": txh_hex if txh_hex.startswith("0x") else "0x" + txh_hex,
                    "network": NETWORK, "settled": settled}
            self.send_response(200 if settled else 402)
            self.send_header("payment-response", base64.b64encode(json.dumps(resp).encode()).decode())
            self.end_headers()
            self.wfile.write(b'{"data":"paid resource ok"}')
        except Exception as e:  # noqa: BLE001
            log(f"SETTLE_ERR {type(e).__name__}: {e}")
            self.send_response(500)
            self.end_headers()
            self.wfile.write(str(e).encode())


# wait for the relayer to be ETH-funded before serving
for _ in range(20):
    if w3.eth.get_balance(relayer.address) > 0:
        break
    time.sleep(6)
log(f"SELLER_READY relayer={relayer.address} eth={w3.eth.get_balance(relayer.address)} "
    f"recipient={RECIPIENT} amount={AMOUNT} port={PORT}")
HTTPServer(("127.0.0.1", PORT), H).serve_forever()
