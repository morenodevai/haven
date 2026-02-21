// Pool of Web Workers for parallel AES-GCM operations

interface PendingOp {
    resolve: (result: ArrayBuffer) => void;
    reject: (error: Error) => void;
}

export class CryptoWorkerPool {
    private workers: Worker[] = [];
    private pending = new Map<number, PendingOp>();
    private nextId = 0;
    private nextWorker = 0;

    constructor(size?: number) {
        const poolSize = size ?? Math.min(navigator.hardwareConcurrency || 4, 8);
        for (let i = 0; i < poolSize; i++) {
            const worker = new Worker(
                new URL('./crypto-worker.ts', import.meta.url),
                { type: 'module' }
            );
            worker.onmessage = (e) => {
                const { id, result, error } = e.data;
                const op = this.pending.get(id);
                if (!op) return;
                this.pending.delete(id);
                if (error) {
                    op.reject(new Error(error));
                } else {
                    op.resolve(result);
                }
            };
            this.workers.push(worker);
        }
    }

    encrypt(key: CryptoKey, data: ArrayBuffer, chunkIndex: number): Promise<ArrayBuffer> {
        return this.dispatch("encrypt", key, data, chunkIndex);
    }

    decrypt(key: CryptoKey, data: ArrayBuffer, chunkIndex: number): Promise<ArrayBuffer> {
        return this.dispatch("decrypt", key, data, chunkIndex);
    }

    private dispatch(
        op: "encrypt" | "decrypt",
        key: CryptoKey,
        data: ArrayBuffer,
        chunkIndex: number
    ): Promise<ArrayBuffer> {
        return new Promise((resolve, reject) => {
            const id = this.nextId++;
            this.pending.set(id, { resolve, reject });

            const worker = this.workers[this.nextWorker % this.workers.length];
            this.nextWorker++;

            // Transfer the ArrayBuffer to the worker (zero-copy)
            worker.postMessage({ id, op, key, data, chunkIndex }, [data]);
        });
    }

    terminate() {
        for (const w of this.workers) w.terminate();
        this.workers = [];
        for (const [, op] of this.pending) {
            op.reject(new Error("Worker pool terminated"));
        }
        this.pending.clear();
    }
}
