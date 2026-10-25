[Documentation](README.md) · [Front page](../README.md)

# Remote control from a phone

The forkdeck Android app shows the job list and each job's tree as it runs,
and can start a task, reply to one, cancel, pause polling or poll now. It
talks to forkdeck directly over the local network: there is no relay and
no account.

```toml
[remote]
enabled = true
listen = "0.0.0.0:7443"
```

Restart forkdeck, then pair a phone:

```
forkdeck remote pair            # shows a QR code; scan it in the app
forkdeck remote devices         # paired phones
forkdeck remote revoke pixel    # unpair one; it is disconnected within seconds
```

The connection is TLS in both directions. forkdeck keeps its own certificate
authority in `~/.config/forkdeck/remote` and trusts nothing else; the QR code
carries its fingerprint and a one-time code. When the phone pairs it makes a
key that never leaves it and gets a certificate signed for that key. A
connection without a paired certificate can do nothing but pair.

A code works once, for two minutes, and only from the subnet it was made for:
the phone has to be on the same network as this machine, not routed in from
elsewhere. With `lan_only = true` (the default) every connection is held to
that, not only pairing. Use `--iface` when the machine has more than one
network.

A paired phone can start agents that edit files in `workdir`: treat it like
a terminal on this machine. Tasks from the phone run in `[remote] workdir`, or
the directory forkdeck was started in.
