/**
 * Wallet management — secp256k1 keypair generation and signing via viem.
 */

import {
  type Hex,
  createWalletClient,
  http,
  type WalletClient,
} from "viem";
import {
  type PrivateKeyAccount,
  privateKeyToAccount,
  generatePrivateKey,
} from "viem/accounts";
import { base, baseSepolia, mainnet } from "viem/chains";
import { ChainId, type WalletInfo } from "./types.js";

const CHAIN_MAP = {
  [ChainId.Base]: base,
  [ChainId.BaseSepolia]: baseSepolia,
  [ChainId.Ethereum]: mainnet,
} as const;

export class HaimaWallet {
  private account: PrivateKeyAccount;
  private client: WalletClient;
  private _chain: ChainId;

  constructor(privateKey?: Hex, chain: ChainId = ChainId.Base) {
    const key = privateKey ?? generatePrivateKey();
    this.account = privateKeyToAccount(key);
    this._chain = chain;
    this.client = createWalletClient({
      account: this.account,
      chain: CHAIN_MAP[chain],
      transport: http(),
    });
  }

  get address(): Hex {
    return this.account.address;
  }

  get chain(): ChainId {
    return this._chain;
  }

  get info(): WalletInfo {
    return { address: this.account.address, chain: this._chain };
  }

  /** EIP-191 personal_sign. */
  async signMessage(message: string): Promise<Hex> {
    return this.account.signMessage({ message });
  }

  /**
   * Sign a payment and return a base64-encoded PAYMENT-SIGNATURE header.
   * Matches the Rust `PaymentSignatureHeader` format.
   */
  async signPaymentHeader(params: {
    scheme: string;
    network: string;
    resourceUrl: string;
    amount: string;
    recipient: string;
  }): Promise<string> {
    const msg = `${params.resourceUrl}:${params.amount}:${params.recipient}`;
    const signature = await this.signMessage(msg);

    const header = {
      scheme: params.scheme,
      network: params.network,
      payload: signature,
    };
    return btoa(JSON.stringify(header));
  }

  /** Load wallet from HAIMA_PRIVATE_KEY environment variable. */
  static fromEnv(chain: ChainId = ChainId.Base): HaimaWallet {
    const key = process.env.HAIMA_PRIVATE_KEY;
    if (!key) {
      throw new Error("HAIMA_PRIVATE_KEY environment variable not set");
    }
    return new HaimaWallet(key as Hex, chain);
  }

  /** Generate a new wallet with a random keypair. */
  static generate(chain: ChainId = ChainId.Base): HaimaWallet {
    return new HaimaWallet(undefined, chain);
  }
}
