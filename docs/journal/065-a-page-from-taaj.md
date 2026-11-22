# A page from TheTaaJ

*Journal entry #65 · Era 4, step C. A web server: `httpd`, a program on the disk that reads HTTP requests off TCP connections and answers them with files from the disk. The parser is a module of the `net` crate with no machine in it, tested on the host; the program gets the network and the file service's reading channel, and nothing else. `curl` on the host fetches the page byte for byte, as a browser would. This ends Era 4.*

```
> httpd &
[1] httpd
httpd: serving /www on port 80
httpd: 10.0.2.2 "GET /" 200 1974 bytes
httpd: 10.0.2.2 "GET /style.css" 200 1374 bytes
httpd: 10.0.2.2 "GET /../welcome" 403 182 bytes
```

```
$ curl -i http://127.0.0.1:8080/
HTTP/1.1 200 OK
Server: TheTaaJ
Content-Type: text/html; charset=utf-8
Content-Length: 1974
Connection: close

<!doctype html>
...
```

## Why

Era 4's goal was "TheTaaJ serves a page of thejat.in itself". Step A gave the machine an address, step B gave it connections. A web server is the first thing on top of TCP that an ordinary program on another machine - a browser, `curl` - can talk to without knowing anything about TheTaaJ. So it tests every layer below it at once: the disk, TaajFS, the file service, the network service, TCP, IPv4, ARP, the card's driver. If the page arrives whole, they all work together.

## HTTP in one picture

HTTP/1.1 is text. The client opens a TCP connection and writes a *request*: a line saying what it wants, some header lines, and an empty line.

```
GET /index.html HTTP/1.1\r\n          request line: method, target, version
Host: 127.0.0.1:8080\r\n              headers: name, colon, value
User-Agent: curl/8.5.0\r\n
Accept: */*\r\n
\r\n                                  empty line: the head is over
```

The server answers in the same shape - a status line, headers, an empty line - and then the *body*, which here is the file's bytes:

```
HTTP/1.1 200 OK\r\n
Server: TheTaaJ\r\n
Content-Type: text/html; charset=utf-8\r\n      what the bytes are
Content-Length: 1974\r\n                        how many follow
Connection: close\r\n                           and then the connection ends
\r\n
<!doctype html>...                              1974 bytes
```

Two headers do the real work. `Content-Length` says where the body ends, so the client knows whether it got all of it. `Content-Type` tells a browser whether to show the bytes as a page, a style sheet, an image, or offer to save them.

The whole conversation:

```
   curl (host)                        httpd (TheTaaJ)
       │  SYN ───────────────────────▶   │   accept_until
       │  ◀──────────────────── SYN+ACK  │
       │  GET / HTTP/1.1 ...\r\n\r\n ──▶ │   read_head: up to the empty line
       │                                 │   parse, path, find (stat)
       │  ◀────── HTTP/1.1 200 OK ...    │   response_head
       │  ◀────── <!doctype html>...     │   read_from, write, a message at a time
       │  ◀──────────────────────── FIN  │   shutdown
       │  FIN ───────────────────────▶   │   linger: read until the client's FIN
       │                                 │   drop: the connection is closed
```

## The parser: `net::http`, with no machine in it

Like TCP in the last entry, everything about HTTP that can be decided without a connection or a disk is in a module of the `net` crate (`net/src/http.rs`), so it runs as ordinary tests on the host. Five functions:

| Function | Does |
|---|---|
| `head_length(bytes)` | where the head ends in what has arrived so far, or `None` if the empty line has not come yet |
| `parse(head)` | the request line and headers, as a `Request` whose parts point into the bytes; or the status to answer with |
| `path(target)` | the target turned into a file name inside the site - or `403` / `400` |
| `content_type(name)` | the Content-Type, from the file's extension |
| `response_head(status, type, length)` | the status line and headers, ending in the empty line |

### Borrowing instead of copying

`Request` is declared with a *lifetime*:

```rust
pub struct Request<'a> {
    pub method: Method,
    pub method_name: &'a str,
    pub target: &'a str,
    pub minor: u8,
    headers: &'a str,
}
```

`'a` is a name for "as long as the bytes the request was read from". `method_name`, `target` and `headers` are not copies; they are views into those bytes (`&str` is a pointer and a length). Parsing allocates nothing. The compiler checks that no `Request` outlives the buffer it points into: in `httpd`, the head buffer lives for the whole of `serve`, and the request is used only inside it. If I tried to keep a `Request` after the buffer was dropped, it would not compile.

`parse` returns `Result<Request<'_>, Status>`: the error *is* the answer. A request with three spaces in its request line is `Err(Status::BadRequest)`; a version of `HTTP/2.0` is `Err(Status::VersionNotSupported)`; a method nobody has heard of is `Err(Status::NotImplemented)`. The program does not have to translate errors into statuses; it writes the one it was given.

### What a request must be

The checks follow RFC 9112, the HTTP/1.1 message syntax:

- the head is ASCII; anything else (a TLS hello sent to the wrong port, for example) is 400;
- the request line has exactly three parts with one space between each;
- the method is a *token* (letters, digits and a few symbols);
- every header line has a name that is a token, then a colon;
- a header line starting with a space (the old way of folding one header over several lines) is refused - RFC 9112 asks servers to, because two programs that disagree about folding can be tricked into reading different requests from the same bytes;
- HTTP/1.1 requires a `Host` header; HTTP/1.0 does not.

`head_length` accepts lines ending in a bare `\n` as well as `\r\n`. The RFC allows a server to, and it lets me type a request by hand with `nc`.

### The path: the one thing that must not go wrong

The target `/about/index.html` becomes the file `www/about/index.html` on the disk. If the target could be `/../welcome`, the server would hand out a file outside the site. That is the oldest web server bug there is ("directory traversal"). `path()` refuses every way out:

```
"/index.html"           → "index.html"
"/about/"               → "about"           (a directory: its index.html)
"/a%20file.txt"         → "a file.txt"      (%xx is a byte, written in hex)
"/page?x=1"             → "page"            (the query is not part of the file)
"/../welcome"           → 403
"/%2e%2e/welcome"       → 403               (.. written in hex: decoded first, then checked)
"//index.html"          → 403               (an empty part)
"/./index.html"         → 403
"/a%00b"                → 400               (a zero byte)
"/a%5c..%5cb"           → 400               (backslashes)
```

The order matters. Percent-decoding comes *first* and the check for `..` *after*. If I checked first, `%2e%2e` would pass the check as harmless text and become `..` afterwards. A boot mutant (below) shows what the check protects: with `path()` skipped, the file service's own path resolution turns `www/../welcome` into `welcome`, and the server hands out the file outside the site.

## The program: `httpd`

`user/src/bin/httpd.rs` is the loop around the parser:

```
httpd [PORT [DIRECTORY [COUNT]]]      default: 80, /www, for ever
```

For each connection, in turn:

1. **Read the head** (`read_head`): read into an 8 KiB buffer until `head_length` finds the empty line. A full buffer with no empty line is `431 Request Header Fields Too Large`. The whole head has to arrive within 10 seconds; one deadline for all of it, not one per read, so a client sending a byte every nine seconds cannot hold the server for ever. If the time runs out with part of a request, the answer is `408 Request Timeout`. With nothing at all, there is no one to answer and the connection is just closed.
2. **Parse it**, and refuse anything but GET and HEAD with `405`, which includes `Allow: GET, HEAD` as HTTP asks.
3. **Find the file**: `path()`, then the site's directory in front, then `stat`. A directory means its `index.html`. "No such file" (or a name the file service refuses) is `404`.
4. **Write the head**, and for GET the body: `read_from` gives the file a message at a time (1023 bytes), each piece goes straight into `write`. The file is never held whole in memory, so a 1 MiB file costs no more memory than a small one.
5. **Finish**: `shutdown` (our FIN), then keep reading what the client sends for up to 2 seconds, then close.

### Why linger before closing

If a connection is closed while bytes the client sent are still unread in it, my TCP (`Connection::close`, last entry) sends a reset instead of a FIN, as RFC 9293 asks. A reset makes the client's TCP (Linux's, on this host) throw away whatever it has received but the client program has not read yet: the answer. So `finish` reads (and discards) until the client's FIN, or 2 seconds.

When does that happen? Not as often as I first thought. The network service moves arriving bytes out of TCP into the program's channel as long as the channel has room, so a few hundred unread bytes are in the channel, not in TCP, and closing is a clean FIN. It happens when the client sends more than the channel holds: a POST with a big body, answered `405` after the head, with the body still coming. A mutant found this for me (see below).

### One connection at a time

`httpd` serves connections in order. While it serves one, the network service holds up to four more in the listener's backlog (`TCP_BACKLOG` from the last entry). The test's three simultaneous `curl`s are served one after the other, and all three get the page. A slow client delays the others by at most the 10-second head limit plus the time to send the file. Serving several at once needs either threads or waiting on several channels at once in one program. That is a later step, not needed for a page.

### What it may touch

The shell decides what services a program gets by name. `httpd` is in a new list:

```rust
/// And these are given both: the network, and the files to read only -
/// a web server, which gives files out and must never change one.
const WEB_PROGRAMS: &[&str] = &["httpd"];
...
Grant::Usual if on_disk(WEB_PROGRAMS) => &[("net", "net"), ("files", "files.look")],
```

`files.look` is the file service's reading channel: requests to change a file arrive on it and are refused by the service. Whatever bug there is in the request handling, a request cannot change the disk. The boot log shows the grant: `[registry] shell gave httpd: net files=files.look`.

## The site

The page is `user/www/index.html`, with `style.css` and `about/index.html` beside it. It is a short page about TheTaaJ that says it was served by TheTaaJ, with a diagram of the path its bytes took. The `Makefile` puts the directory on the disks as `www` (`put-site`): on `make run64`'s and `make run-aarch64`'s disks, and on the HTTP test's disk. The other test disks do not get it, because their tests count the directories at the root.

The page is written for this step, not copied from www.thejat.in. Any static page works the same way: put its files in `user/www` (or on the disk's `www` with `tfstool put`) and `httpd` serves them. Pages that need a program on the server to build them (search, comments) do not.

## Testing against clients that are not mine

`make http64-test` and `make aarch64-http-test` boot the machine with its port 80 passed to a port of the host, and type:

```
httpd 0                  usage: no port 0
httpd 80 nosuch          httpd: /nosuch: no such file or directory
httpd 80 welcome         httpd: /welcome: not a directory
httpd 80 /www 19         nineteen connections, then it ends
netinfo
```

Meanwhile `http-host.py` on the host makes the nineteen connections, most of them with `curl`, so the requests come from an HTTP client I did not write. The odd ones go through a plain socket, because curl will not send a broken request:

| Connection | Expected |
|---|---|
| `curl /`, `/style.css`, `/about/`, `/about` | 200, the file byte for byte, the right Content-Type |
| `HEAD /` | 200, `Content-Length: 1974`, nothing after the head |
| `curl /nothing.html` | 404 |
| `GET /../welcome`, `GET /%2e%2e/welcome` | 403, and not the file's contents |
| `curl -X POST` | 405 with `Allow: GET, HEAD` |
| POST with a 300 KB body | 405, arriving whole: the server reads the body to the end before closing |
| `hello` | 400 |
| `GET /style.css HTTP/1.0` with no Host | 200 |
| `curl /big.bin` | 1 MiB of random bytes, identical |
| three `curl /` at once | all three 200 and whole |
| half a request, then silence | 408, after 8-20 seconds |
| 9000 bytes of head | 431 |
| connect and close at once | nothing; httpd says `sent no request` |

The machine's own log must show each answer (`httpd: 10.0.2.2 "GET /big.bin" 200 1048576 bytes`), and `httpd` must exit with code 0 after the nineteenth.

## Design decisions

| Decision | Chosen | Rejected, and why |
|---|---|---|
| Where HTTP lives | A module of the `net` crate, sans-IO like TCP | Inside `httpd`: untestable without booting. A new crate: the user crate already depends on `net`, and HTTP is one more protocol in the stack |
| Requests per connection | One, then `Connection: close` | Keep-alive: saves a TCP handshake per file, but needs to know where each request's body ends, and a server serving one connection at a time would be held by an idle browser |
| Connections at once | One; others wait in the backlog | Several, by waiting on all their channels at once (`object_wait`) or with a thread each: more code than a page needs, and the backlog already keeps the waiting clients. Later |
| Reading the file | `read_from`, a message at a time, straight into the connection | Reading the whole file first: memory proportional to the file, and the first byte only after the last is read |
| A directory | Its `index.html` | A generated listing: a server that lists directories shows what was never meant to be linked |
| `/about` without a slash | Served as `/about/` directly | A redirect to `/about/`: what bigger servers do, so relative links resolve right; my pages use absolute links (`/style.css`), so it is not needed yet |
| The request time limit | 10 s for the whole head | Per read: a client sending one byte every few seconds would never time out |
| Unknown methods | 501; known ones not done here 405 | Everything 400: hides from the client whether it made a mistake or asked for something this server does not do |
| Files | The reading channel only | The full channel: a bug in the request handling could then change the disk |

## Bugs

- **None in the program found by the tests**: both boot tests passed on their first run. That made me suspicious, so I read the boot log line by line - each answer is there with the right status and length - and ran mutants to make sure the tests can fail.
- **A test that did not test what I thought.** I believed the 431 check (9000 bytes of head, 8192 read) covered the linger before closing. Removing the linger changed nothing: the 800 unread bytes were sitting in the program's channel, not in TCP, so the close was a clean FIN either way. I added a POST with a 300 KB body. It fills the channel and TCP's buffer behind it, and without the linger the client's TCP gets a reset and throws the 405 away. With the linger the 405 arrives.
- **curl tidies paths.** `curl http://127.0.0.1:8080/../welcome` sends `GET /welcome`, not `GET /../welcome` (the log showed a 404 for `/welcome`). To send `..` to the server, curl needs `--path-as-is`. The automated test sends those requests through a plain socket, so it was not fooled. But the testing note's first draft would have been.

## How I checked it

Automated, on the host:

- `net`: 78 tests (72 before). The six new ones: where a head ends (cut at every byte of a curl request, and with bare `\n`); a curl request read, headers looked up in any case; fifteen kinds of broken request, each with its status; thirteen targets turned into names and sixteen refused (every `..`, `.`, empty part, encoded and not); content types; response heads, and that every status has its own words.

Automated, by booting:

- `make http64-test` and `make aarch64-http-test`: both pass, `host: ok: 17 checks`, nineteen connections.

Mutation experiments: 20 mutants of `http.rs` on the host - the bare `\n` line end, every part of the path check (`..`, `.`, empty parts, backslash, the order of the hex digits, the query and the fragment, the absolute form), the Host requirement, header names as tokens, the four-part request line, header lookup in any case and trimmed values, empty lines before the request, `Allow`, content types and their case, ASCII only, lower-case hex, and the version check. 19 caught. One survived and is equivalent: trimming spaces around a header's *name*, which `parse` has already refused if it has spaces. Five mutants of `httpd`, by booting: a body sent to HEAD (caught), a directory not answered with its `index.html` (caught), the path check skipped (caught - the file outside the site was served, because the file service resolves `www/../welcome` to `welcome`), no linger before closing (survived; caught after the new POST check), and the file sent in pieces of 100 bytes (survived, equivalent: the same bytes, only slower).

By hand: every step of the testing note below, with scripted boots (keys typed through QEMU's monitor, `curl` on the host).

Not tested: a real browser (the page and style sheet are checked as bytes and as Content-Type, not looked at by me in a browser); a real network; clients that send a body with GET; more than three clients at once; a file changing while it is sent (the length sent is the length at `stat`, and the body is cut there).

## What is still missing

- Several connections at once, and keep-alive.
- HTTPS: TLS needs cryptography and a source of randomness, neither of which TheTaaJ has.
- Redirects, `If-Modified-Since` / `304`, ranges (`206`), compression.
- Files larger than TaajFS's limit of about 4 MB.
- The real pages of www.thejat.in in `user/www`: the server is ready for them, but the files are not copied in.
- Names (DNS) and an address from the network (DHCP): the machine is reachable only because QEMU passes a port to it.

## What I learned

- HTTP/1.1 is small at its core: a line, headers, an empty line, and `Content-Length`. Almost everything else in the RFCs is about doing more than one request per connection, caching, and the cases where two programs could read the same bytes differently.
- A lifetime is how Rust lets a parser hand out pieces of its input without copying: `Request<'a>` cannot outlive the bytes it points into, and the compiler checks that.
- The order of decoding and checking decides whether a path check works. Decode first, check after.
- Least privilege is cheap when the operating system already has the pieces: giving `httpd` the reading channel instead of the full one was one line in the shell, and it makes a whole class of bugs harmless.
- Closing a connection is part of the answer. Without the linger, a client that sent more than was read can lose the response to a reset.
- A test can pass for a reason other than the one I meant. The 431 check passed with and without the linger, because of a buffer I had forgotten was there. Only the mutant showed it.
- Clients rewrite what they are told to send: curl removes `..` from a URL before sending it. To test a server's defences, the bad request has to go through a plain socket.

## Testing note

**1. The page, from the PC**

Start the machine with the host's port 8080 passed on to the machine's port 80. (`make run64` puts the site on its disk as `www`, beside the programs.)

```
make run64 RUN_NET="-netdev user,id=n0,hostfwd=tcp:127.0.0.1:8080-:80 -device virtio-net-pci,netdev=n0"
```

| Type (in QEMU) | You should see |
|---|---|
| `httpd &` | `[1] httpd`, and `httpd: serving /www on port 80` |

On the host, in another terminal:

| Type (on the host) | You should see |
|---|---|
| `curl -i http://127.0.0.1:8080/` | `HTTP/1.1 200 OK`, `Server: TheTaaJ`, `Content-Type: text/html; charset=utf-8`, `Content-Length: 1974`, `Connection: close`, an empty line, and the page; in QEMU: `httpd: 10.0.2.2 "GET /" 200 1974 bytes` |
| `curl -s http://127.0.0.1:8080/ \| cmp - user/www/index.html && echo same` (in the TheTaaJ directory) | `same` |
| `curl -s http://127.0.0.1:8080/style.css \| cmp - user/www/style.css && echo same` | `same` |
| `curl -I http://127.0.0.1:8080/about/` | `HTTP/1.1 200 OK` and `Content-Length: 1549`, no page (HEAD); in QEMU: `"HEAD /about/" 200 0 bytes` |
| `curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8080/nothing` | `404` |
| `curl --path-as-is -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8080/../welcome` | `403`; in QEMU: `"GET /../welcome" 403 182 bytes` |
| `curl -i -X POST -d x=1 http://127.0.0.1:8080/` | `HTTP/1.1 405 Method Not Allowed` and `Allow: GET, HEAD` |
| `printf 'GET / HTTP/1.0\r\n\r\n' \| nc -N 127.0.0.1 8080 \| head -1` | `HTTP/1.1 200 OK` |

| Look for | Meaning |
|---|---|
| without `--path-as-is`, the `..` request is a `404`, and QEMU logs `"GET /welcome"` | curl removed the `..` itself before sending; the server never saw it |

And in a browser on the host: open `http://127.0.0.1:8080/`.

| Look for | Meaning |
|---|---|
| the page "TheTaaJ", in a serif font with a diagram in a grey box; "About this server" leads to a page with a table | the page and its style sheet both came from the machine |
| in QEMU: `"GET /" 200 1974 bytes` and `"GET /style.css" 200 1374 bytes` (a browser may also ask for `/favicon.ico`: `404`) | |

(I have not looked at the page in a browser myself: this row is what it should show.)

**2. The same on AArch64**

```
make run-aarch64 RUN_A64_DEVICES="-drive file=build/aarch64/taaj-disk.img,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0 -netdev user,id=n0,hostfwd=tcp:127.0.0.1:8080-:80 -device virtio-net-device,netdev=n0"
```

The terminal is the keyboard and screen. To leave QEMU: **Ctrl+A, then X**.

| Type | You should see |
|---|---|
| `httpd &` | `[1] httpd`, `httpd: serving /www on port 80` |

| Type (on the host, another terminal, in the TheTaaJ directory) | You should see |
|---|---|
| `curl -s http://127.0.0.1:8080/ \| head -5` | `<!doctype html>`, `<html lang="en">`, `<head>`, ... |
| `curl -s http://127.0.0.1:8080/ \| cmp - user/www/index.html && echo same` | `same`; in QEMU: `httpd: 10.0.2.2 "GET /" 200 1974 bytes` |

**3. The tests by themselves**

```
make http64-test
make aarch64-http-test
```

Each prints the host's verdict and ends `http64-test passed` / `aarch64-http-test passed`:

| Look for | Meaning |
|---|---|
| `host: ok: 17 checks` | every one of the nineteen connections was answered as it should be, the 1 MiB file byte for byte |

The machine's side is in `build/http64.log` (and `build/aarch64/http.log`): `grep -a 'httpd:' build/http64.log` shows one line per connection, ending with `httpd: 10.0.2.2 sent no request`.

Host checks:

```
cd net && cargo test           # 78 tests: the protocols, TCP, the stack and HTTP
```

Automated: `make test64`
