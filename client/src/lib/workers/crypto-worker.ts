// Web Worker for AES-GCM encryption/decryption
// Offloads crypto from the main thread for higher throughput

interface WorkerRequest {
    id: number;
    op: "encrypt" | "decrypt";
    key: CryptoKey;
    data: ArrayBuffer;
    chunkIndex: number;
}

interface WorkerResponse {
    id: number;
    result?: ArrayBuffer;
    error?: string;
}

function makeNonce(chunkIndex: number): Uint8Array {
    const nonce = new Uint8Array(12);
    const view = new DataView(nonce.buffer as ArrayBuffer);
    view.setUint32(4, Math.floor(chunkIndex / 0x100000000));
    view.setUint32(8, chunkIndex >>> 0);
    return nonce;
}

self.onmessage = async (e: MessageEvent<WorkerRequest>) => {
    const { id, op, key, data, chunkIndex } = e.data;
    const nonce = makeNonce(chunkIndex);

    try {
        let result: ArrayBuffer;
        if (op === "encrypt") {
            result = await crypto.subtle.encrypt(
                { name: "AES-GCM", iv: nonce as unknown as Uint8Array<ArrayBuffer> },
                key,
                data
            );
        } else {
            result = await crypto.subtle.decrypt(
                { name: "AES-GCM", iv: nonce as unknown as Uint8Array<ArrayBuffer> },
                key,
                data
            );
        }
        (self as any).postMessage({ id, result } as WorkerResponse, [result]);
    } catch (err: any) {
        (self as any).postMessage({ id, error: err.message } as WorkerResponse);
    }
};
