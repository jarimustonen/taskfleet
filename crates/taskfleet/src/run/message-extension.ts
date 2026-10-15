// Bundled Pi adapter. Loaded only by Taskfleet's generated launcher; never from the checkout.
// A response means pi.sendUserMessage accepted a steering message, not that the
// model has read it or complied. No message body is written to Taskfleet logs.
import { createServer } from "node:net";
import { chmodSync, existsSync, unlinkSync } from "node:fs";

export default function (pi: any) {
  const socket = process.env.TASKFLEET_MESSAGE_SOCKET;
  const session = process.env.TASKFLEET_MESSAGE_SESSION;
  if (!socket || !session) return;
  let server: ReturnType<typeof createServer> | undefined;
  pi.on("session_start", async () => {
    if (server) return;
    // An existing path could belong to another live worker. Never steal it.
    if (existsSync(socket)) return;
    const next = createServer((peer) => {
      let input = Buffer.alloc(0);
      peer.setTimeout(3000, () => peer.destroy());
      peer.on("data", (chunk) => {
        input = Buffer.concat([input, chunk]);
        if (input.length > 65536) { peer.destroy(); return; }
        const end = input.indexOf(10); // LF only; Unicode line separators are payload.
        if (end < 0) return;
        peer.removeAllListeners("data");
        try {
          const request = JSON.parse(input.subarray(0, end).toString("utf8"));
          if (request.session !== session || typeof request.message !== "string" ||
              !request.message.trim() || request.message.length > 32768) {
            peer.end(JSON.stringify({ id: request.id, session, accepted: false }) + "\n");
            return;
          }
          pi.sendUserMessage(request.message, { deliverAs: "steer" });
          peer.end(JSON.stringify({ id: request.id, session, accepted: true }) + "\n");
        } catch {
          peer.end(JSON.stringify({ session, accepted: false }) + "\n");
        }
      });
    });
    try {
      await new Promise<void>((resolve, reject) => {
        next.once("error", reject);
        next.listen(socket, () => { next.off("error", reject); resolve(); });
      });
      chmodSync(socket, 0o600);
      server = next;
    } catch {
      next.close();
    }
  });
  pi.on("session_shutdown", async () => {
    if (!server) return;
    server.close();
    server = undefined;
    try { unlinkSync(socket); } catch { /* already removed */ }
  });
}
