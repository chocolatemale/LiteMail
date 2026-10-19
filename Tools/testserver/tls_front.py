#!/usr/bin/env python3
"""TLS front for the test Dovecot.

Dovecot 2.4.5 on macOS drops TLS sessions right after login (its post-login TLS proxy
closes the connection), so Dovecot serves plaintext only and this process adds:

  --tls-port       implicit TLS (IMAPS), forwarded to the plaintext Dovecot port
  --starttls-port  plaintext IMAP that handles STARTTLS itself, then forwards

Both exercise the client's real TLS code paths. Standard library only.
"""

import argparse
import asyncio
import ssl


async def pipe(reader, writer):
    try:
        while True:
            data = await reader.read(65536)
            if not data:
                break
            writer.write(data)
            await writer.drain()
    except (ConnectionError, ssl.SSLError):
        pass
    finally:
        try:
            writer.close()
        except Exception:
            pass


async def splice(creader, cwriter, sreader, swriter):
    await asyncio.gather(pipe(creader, swriter), pipe(sreader, cwriter))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--upstream", type=int, required=True)
    ap.add_argument("--tls-port", type=int, required=True)
    ap.add_argument("--starttls-port", type=int, required=True)
    ap.add_argument("--cert", required=True)
    ap.add_argument("--key", required=True)
    a = ap.parse_args()

    ctx = ssl.create_default_context(ssl.Purpose.CLIENT_AUTH)
    ctx.load_cert_chain(a.cert, a.key)

    async def implicit(creader, cwriter):
        try:
            sreader, swriter = await asyncio.open_connection("127.0.0.1", a.upstream)
            await splice(creader, cwriter, sreader, swriter)
        except (ConnectionError, ssl.SSLError):
            cwriter.close()

    async def starttls(creader, cwriter):
        try:
            sreader, swriter = await asyncio.open_connection("127.0.0.1", a.upstream)
            greeting = await sreader.readline()
            # Dovecot runs without TLS, so advertise STARTTLS on its behalf.
            greeting = greeting.replace(b"[CAPABILITY ", b"[CAPABILITY STARTTLS ", 1)
            cwriter.write(greeting)
            await cwriter.drain()
            # Before TLS, only CAPABILITY / NOOP / STARTTLS are meaningful.
            while True:
                line = await creader.readline()
                if not line:
                    swriter.close()
                    return
                tag, _, rest = line.decode(errors="replace").strip().partition(" ")
                cmd = rest.upper()
                if cmd == "STARTTLS":
                    cwriter.write(f"{tag} OK Begin TLS negotiation now\r\n".encode())
                    await cwriter.drain()
                    await cwriter.start_tls(ctx)
                    break
                if cmd == "CAPABILITY":
                    cwriter.write(b"* CAPABILITY IMAP4rev1 STARTTLS LOGINDISABLED\r\n")
                    cwriter.write(f"{tag} OK CAPABILITY completed\r\n".encode())
                else:
                    cwriter.write(f"{tag} BAD Use STARTTLS first\r\n".encode())
                await cwriter.drain()
            await splice(creader, cwriter, sreader, swriter)
        except (ConnectionError, ssl.SSLError):
            cwriter.close()

    async def run():
        s1 = await asyncio.start_server(implicit, "127.0.0.1", a.tls_port, ssl=ctx)
        s2 = await asyncio.start_server(starttls, "127.0.0.1", a.starttls_port)
        print(f"tls front: {a.tls_port} (TLS), {a.starttls_port} (STARTTLS) -> {a.upstream}", flush=True)
        async with s1, s2:
            await asyncio.gather(s1.serve_forever(), s2.serve_forever())

    asyncio.run(run())


if __name__ == "__main__":
    main()
