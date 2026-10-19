#!/usr/bin/env python3
"""Minimal SMTP server for LiteMail integration tests.

- AUTH PLAIN / LOGIN, password must be "test"
- STARTTLS on the plain port, implicit TLS on the TLS port
- Delivers every message into <root>/home/<recipient>/Maildir/new, where Dovecot picks it up
- Appends one JSON line per accepted message to <root>/smtp-log.jsonl (envelope + size)
- Recipient "reject@localhost" is refused with 550, to exercise error handling

Only the Python standard library is used.
"""

import argparse
import asyncio
import base64
import itertools
import json
import os
import ssl
import time

PASSWORD = "test"
counter = itertools.count()


def deliver(root, rcpt, data):
    box = os.path.join(root, "home", rcpt, "Maildir")
    for sub in ("cur", "new", "tmp"):
        os.makedirs(os.path.join(box, sub), exist_ok=True)
    name = f"{time.time_ns()}.P{os.getpid()}Q{next(counter)}.localhost"
    tmp = os.path.join(box, "tmp", name)
    with open(tmp, "wb") as f:
        f.write(data)
    os.rename(tmp, os.path.join(box, "new", name))


def addr(arg):
    start, end = arg.find("<"), arg.find(">")
    if start == -1 or end == -1:
        return arg.split(":", 1)[-1].strip()
    return arg[start + 1 : end].strip()


class Session:
    def __init__(self, reader, writer, root, ctx, tls):
        self.reader, self.writer, self.root, self.ctx, self.tls = reader, writer, root, ctx, tls
        self.authed = False
        self.reset()

    def reset(self):
        self.mail_from, self.rcpts = None, []

    async def send(self, line):
        self.writer.write((line + "\r\n").encode())
        await self.writer.drain()

    async def multi(self, code, lines):
        for i, l in enumerate(lines):
            sep = " " if i == len(lines) - 1 else "-"
            self.writer.write(f"{code}{sep}{l}\r\n".encode())
        await self.writer.drain()

    async def readline(self):
        line = await self.reader.readline()
        return line.decode("utf-8", "replace").rstrip("\r\n") if line else None

    def check(self, user, password):
        self.authed = password == PASSWORD and bool(user)
        return self.authed

    async def auth(self, arg):
        parts = arg.split()
        mech = parts[0].upper() if parts else ""
        if mech == "PLAIN":
            payload = parts[1] if len(parts) > 1 else None
            if payload is None:
                await self.send("334 ")
                payload = await self.readline()
            try:
                _, user, password = base64.b64decode(payload).decode().split("\0")
            except Exception:
                return await self.send("501 Bad PLAIN payload")
        elif mech == "LOGIN":
            if len(parts) > 1:
                user = base64.b64decode(parts[1]).decode()
            else:
                await self.send("334 VXNlcm5hbWU6")
                user = base64.b64decode(await self.readline() or "").decode()
            await self.send("334 UGFzc3dvcmQ6")
            password = base64.b64decode(await self.readline() or "").decode()
        else:
            return await self.send("504 Unrecognized authentication type")
        if self.check(user, password):
            await self.send("235 2.7.0 Authentication successful")
        else:
            await self.send("535 5.7.8 Authentication credentials invalid")

    async def data(self):
        await self.send("354 End data with <CR><LF>.<CR><LF>")
        chunks = []
        while True:
            line = await self.reader.readline()
            if not line:
                return
            if line in (b".\r\n", b".\n"):
                break
            if line.startswith(b".."):
                line = line[1:]
            chunks.append(line)
        raw = b"".join(chunks)
        for r in self.rcpts:
            deliver(self.root, r, raw)
        with open(os.path.join(self.root, "smtp-log.jsonl"), "a") as f:
            f.write(json.dumps({"from": self.mail_from, "rcpts": self.rcpts, "size": len(raw)}) + "\n")
        self.reset()
        await self.send("250 2.0.0 OK queued")

    async def run(self):
        await self.send("220 localhost ESMTP LiteMail test sink")
        while True:
            line = await self.readline()
            if line is None:
                return
            verb, _, arg = line.partition(" ")
            verb = verb.upper()
            if verb in ("EHLO", "HELO"):
                caps = ["localhost", "8BITMIME", "SMTPUTF8", "SIZE 52428800", "AUTH PLAIN LOGIN"]
                if not self.tls:
                    caps.append("STARTTLS")
                await self.multi(250, caps)
            elif verb == "STARTTLS" and not self.tls:
                await self.send("220 2.0.0 Ready to start TLS")
                await self.writer.start_tls(self.ctx)
                self.tls, self.authed = True, False
                self.reset()
            elif verb == "AUTH":
                await self.auth(arg)
            elif verb == "MAIL":
                if not self.authed:
                    await self.send("530 5.7.0 Authentication required")
                else:
                    self.mail_from = addr(arg)
                    await self.send("250 2.1.0 OK")
            elif verb == "RCPT":
                rcpt = addr(arg)
                if rcpt == "reject@localhost":
                    await self.send("550 5.1.1 Mailbox unavailable")
                else:
                    self.rcpts.append(rcpt)
                    await self.send("250 2.1.5 OK")
            elif verb == "DATA":
                if not self.rcpts:
                    await self.send("503 5.5.1 No recipients")
                else:
                    await self.data()
            elif verb == "RSET":
                self.reset()
                await self.send("250 2.0.0 OK")
            elif verb == "NOOP":
                await self.send("250 2.0.0 OK")
            elif verb == "QUIT":
                await self.send("221 2.0.0 Bye")
                return
            else:
                await self.send("502 5.5.2 Command not implemented")


async def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", required=True)
    ap.add_argument("--port", type=int, required=True)
    ap.add_argument("--tls-port", type=int, required=True)
    ap.add_argument("--cert", required=True)
    ap.add_argument("--key", required=True)
    a = ap.parse_args()

    ctx = ssl.create_default_context(ssl.Purpose.CLIENT_AUTH)
    ctx.load_cert_chain(a.cert, a.key)

    def handler(tls):
        async def h(reader, writer):
            try:
                await Session(reader, writer, a.root, ctx, tls).run()
            except (ConnectionError, ssl.SSLError, asyncio.IncompleteReadError):
                pass
            finally:
                writer.close()

        return h

    plain = await asyncio.start_server(handler(False), "127.0.0.1", a.port)
    secure = await asyncio.start_server(handler(True), "127.0.0.1", a.tls_port, ssl=ctx)
    print(f"smtp sink on {a.port} (STARTTLS) and {a.tls_port} (TLS)", flush=True)
    async with plain, secure:
        await asyncio.gather(plain.serve_forever(), secure.serve_forever())


if __name__ == "__main__":
    asyncio.run(main())
