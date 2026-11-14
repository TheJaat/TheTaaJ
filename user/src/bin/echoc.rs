//! echod's client - and a test, from ring 3, of every rule of handles and
//! channels. Each check is something a hostile program would try.

#![no_std]
#![no_main]

use taaj::{println, rights, Error, Handle};

taaj::program!(main);

/// Fails the test (exit code = line number) unless `$cond` holds.
macro_rules! check {
    ($cond:expr, $what:literal) => {
        if !$cond {
            println!("[echoc] FAILED: {}", $what);
            return line!() as i64;
        }
    };
}

/// Sends `text` on `channel` and returns the reply's length in `buffer`.
fn ask(channel: Handle, text: &[u8], buffer: &mut [u8]) -> Result<usize, Error> {
    taaj::send(channel, text, &[])?;
    Ok(taaj::receive(channel, buffer, &mut [])?.0)
}

fn main() -> i64 {
    let boot = taaj::bootstrap();
    let mut buffer = [0u8; 64];

    // 1. A request and its reply.
    let n = ask(boot, b"hello, echod", &mut buffer);
    check!(n.map(|n| &buffer[..n]) == Ok(b"HELLO, ECHOD"), "echo over the bootstrap channel");
    println!("[echoc] echod replied {:?}", core::str::from_utf8(&buffer[..12]).unwrap_or("?"));

    // 2. Handles cannot be forged: numbers this process was never given
    //    name nothing - including the bootstrap handle's slot with the
    //    wrong generation.
    for forged in [0, boot + 1, boot ^ 1 << 32, u64::MAX] {
        check!(taaj::send(forged, b"x", &[]) == Err(Error::InvalidHandle), "forged handle accepted");
    }

    // 3. Rights: a receive-only copy cannot send, cannot be copied
    //    (no DUPLICATE), and once closed is dead - even for receiving.
    let Ok(receive_only) = taaj::duplicate(boot, rights::RECEIVE) else { return 100 };
    check!(taaj::send(receive_only, b"x", &[]) == Err(Error::AccessDenied), "sent without SEND");
    check!(taaj::duplicate(receive_only, rights::ALL) == Err(Error::AccessDenied), "rights widened");
    check!(taaj::close(receive_only) == Ok(()), "close");
    check!(taaj::close(receive_only) == Err(Error::InvalidHandle), "closed twice");
    check!(taaj::duplicate(boot, 1 << 20).is_err(), "made-up right accepted");

    // 4. A channel end cannot be sent through its own channel (it would
    //    keep itself alive forever).
    check!(taaj::send(boot, b"", &[boot]) == Err(Error::InvalidArgument), "sent a channel through itself");

    // 5. Backpressure and message boundaries, on a channel of our own:
    //    the inbox holds 32 messages, then sends fail; a message too big
    //    for the buffer stays queued instead of being cut or lost.
    let Ok((a, b)) = taaj::channel_create() else { return 101 };
    for n in 0..abi::CHANNEL_QUEUE_MAX {
        check!(taaj::send(a, &[n as u8; 5], &[]) == Ok(()), "send within the limit");
    }
    check!(taaj::send(a, b"one more", &[]) == Err(Error::QueueFull), "the queue grew without bound");
    let mut small = [0u8; 2];
    check!(taaj::receive(b, &mut small, &mut []) == Err(Error::BufferTooSmall), "small buffer");
    check!(taaj::receive(b, &mut buffer, &mut []) == Ok((5, 0)), "the message stayed queued");
    check!(buffer[..5] == [0; 5], "the first message first");
    check!(taaj::receive(b, &mut buffer, &mut []) == Ok((5, 0)) && buffer[0] == 1, "in order");
    check!(taaj::receive(0x7000_0000_0000, &mut buffer, &mut []) == Err(Error::InvalidHandle), "bad handle");
    // A buffer in unmapped memory: an error, and the message survives.
    check!(
        taaj::syscall5(taaj::call::CHANNEL_RECEIVE, b, 0x7000_0000_0000, 64, 0, 0)
            == abi::encode(Err(Error::BadAddress)),
        "receive into unmapped memory"
    );
    check!(taaj::receive(b, &mut buffer, &mut []) == Ok((5, 0)) && buffer[0] == 2, "still queued");
    check!(taaj::close(a) == Ok(()) && taaj::close(b) == Ok(()), "close the pair");
    println!("[echoc] forged handles, missing rights, a full queue and bad buffers were all refused");

    // 6. Passing a capability: a new channel, one end sent to echod. The
    //    handle leaves this process - it is echod's now.
    let Ok((mine, theirs)) = taaj::channel_create() else { return 102 };
    check!(taaj::send(boot, b"", &[theirs]) == Ok(()), "sending a handle");
    check!(taaj::close(theirs) == Err(Error::InvalidHandle), "a sent handle stayed behind");
    let n = taaj::receive(mine, &mut buffer, &mut []);
    check!(n.map(|(n, _)| &buffer[..n]) == Ok(b"welcome"), "greeting on the private channel");
    let n = ask(mine, b"private line", &mut buffer);
    check!(n.map(|n| &buffer[..n]) == Ok(b"PRIVATE LINE"), "echo on the private channel");
    check!(taaj::close(mine) == Ok(()), "hang up");
    println!("[echoc] handed echod a private channel and talked over it");

    // 7. Closing must not recurse: a chain of 2000 channels, each end
    //    queued in the next one's inbox, held by a single handle. Closing
    //    it destroys all 2000 - without blowing the kernel stack.
    let mut chain: Handle = 0;
    for _ in 0..2000 {
        let Ok((x, y)) = taaj::channel_create() else { return 103 };
        if chain != 0 {
            check!(taaj::send(x, b"", &[chain]) == Ok(()), "nest");
        }
        check!(taaj::close(x) == Ok(()), "close x");
        chain = y;
    }
    check!(taaj::close(chain) == Ok(()), "close the chain");
    println!("[echoc] built and destroyed a chain of 2000 nested channels");

    // 8. The service crashes. The kernel kills it and closes its handles;
    //    this client gets PeerClosed - an error to handle, not a hang and
    //    not a crash of its own.
    check!(taaj::send(boot, b"crash", &[]) == Ok(()), "send crash");
    check!(taaj::receive(boot, &mut buffer, &mut []) == Err(Error::PeerClosed), "a dead service");
    println!("[echoc] echod died; the channel reported PeerClosed and echoc carried on");
    0
}
