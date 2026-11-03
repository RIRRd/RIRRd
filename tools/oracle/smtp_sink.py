#!/usr/bin/env python3
"""Loopback-only test SMTP sink. Discards messages and never relays or logs content."""
import asyncio

async def client(reader, writer):
    async def send(line):
        writer.write(line + b'\r\n')
        await writer.drain()
    try:
        await send(b'220 synthetic-oracle SMTP sink')
        while line := await asyncio.wait_for(reader.readline(), 15):
            command = line.split(b' ', 1)[0].strip().upper()
            if command in (b'EHLO', b'HELO'):
                await send(b'250 synthetic-oracle')
            elif command in (b'MAIL', b'RCPT', b'RSET', b'NOOP'):
                await send(b'250 OK')
            elif command == b'DATA':
                await send(b'354 End with dot')
                size = 0
                while True:
                    data = await asyncio.wait_for(reader.readline(), 15)
                    size += len(data)
                    if not data or size > 2 * 1024 * 1024:
                        return
                    if data in (b'.\r\n', b'.\n'):
                        break
                await send(b'250 Discarded in test sink')
            elif command == b'QUIT':
                await send(b'221 Bye')
                return
            else:
                await send(b'502 Unsupported command')
    except (TimeoutError, ValueError, ConnectionError):
        pass
    finally:
        writer.close()
        await writer.wait_closed()

async def main():
    server = await asyncio.start_server(client, '127.0.0.1', 1025, limit=65536)
    async with server:
        await server.serve_forever()

asyncio.run(main())
