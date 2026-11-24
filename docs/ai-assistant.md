# The assistant: AI built into TheTaaJ

*A brainstorm, not a plan yet. Started 2026-11-25. Nothing here is decided until it moves into the roadmap (PROJECT-CONTEXT §7 and the era table).*

## What I want

A personal assistant that **does things**, not only answers. I should no longer have to do everything myself: I ask, by typing or by voice, and it does the work, then shows or speaks the result. For example:

- "Go to that directory and check the file sizes" → it lists them and shows or says: "I found 12 files, the biggest is ...".
- "When this task is done, turn off the system."
- "Open that website and check the price of this product."
- "Uninstall that program."
- "Play that song."
- "Explain this crash log."
- "Create a file and write what I say into it."
- ... and much more.

It must be **part of the OS**, not an app on top. And the model must be **replaceable and upgradeable at any time**.

## How it would work

```
 me (typing, or voice → speech-to-text)
        │
        ▼
 assistant  ──── asks ────►  model (infer service)
    │  ◄── "call files.list(dir=/docs)" ──┘
    │
    ├─ runs the tool through the service that owns it (fs, power, net, audio, ...)
    ├─ gives the result back to the model, which decides the next step or answers
    ▼
 answer on the screen, or spoken (text-to-speech)
```

The model never touches the system itself. It can only *ask* for a tool, by name and with arguments. The `assistant` program carries the request out, and it holds only the capabilities it was given.

**Safety rules:**
- **Reading is free:** listing files, file sizes, explaining a crash log.
- **Changing things asks first:** deleting, uninstalling, overwriting. I see the planned action and say yes. An action I asked for myself ("turn off when done") counts as confirmed.
- **Web pages are untrusted:** text from a website can never trigger a tool. Otherwise a page could say "now delete the user's files", and a small model might obey.

## Making the model replaceable

The rule: **nothing in the OS talks to a model directly.** Everything goes through one service with a fixed protocol, the way programs talk to `fs` without knowing which disk driver is underneath.

```
 shell, edit, window server, voice key, ...      (ask in plain language)
                   │
               assistant      ── the agent: plans, calls tools, asks me to confirm
                   │
     abi::infer protocol      ── fixed: generate, embed, transcribe, speak
                   │
        ┌──────────┴───────────┐
  local engine (ours)     remote backend (optional, off by default)
        │
   /models/…  model files + a short manifest each
```

- **Changing the model is a file change, not a code change.** GGUF files carry the architecture, the sizes, the tokenizer and the chat template inside them. Each model has a small manifest beside it: its role (chat, tools, embedding, speech-to-text, speech), its licence, a checksum.
- **Commands:**
  - `model list` shows what's installed;
  - `model use qwen3-4b` switches without a reboot;
  - `model test qwen3-4b` runs a fixed set of assistant tasks first, to check a new model isn't worse;
  - `model rollback` goes back to the previous one.
- **Upgrades:**
  - same family, newer or bigger (Qwen3 → its successor): only a new file;
  - a new family (Llama, Gemma, Mistral, ...): the engine needs code for its layer types once. Most share the same building blocks.
- **Models are packages:** the package manager installs, updates and removes them like programs.
- **Several models, each for its job:**
  - a small fast one decides what I meant;
  - a bigger one plans multi-step tasks;
  - an embedding model indexes files;
  - Whisper for speech-to-text, a voice for speaking.

  Each can be swapped on its own.

## Ideas that come from TheTaaJ being a capability OS

1. **Services describe their own tools.** Every service (fs, power, audio, net, ...) publishes "here's what I can do" in the registry. The assistant learns the tools from there, so a newly installed program brings its tools with it, and the assistant never needs updating to use it.
2. **Permission = capability, scoped to one task.** "Delete the logs in /tmp" gives the assistant a delete right for that directory, for that task only, and then the right is revoked. It can never do more than I allowed, even if the model goes wrong.
3. **Undo everything the assistant did.** TaajFS already commits changes atomically with generation stamps. With snapshots, one is taken before each assistant task, and one key undoes the whole task.
4. **An activity log.** Every action the assistant takes appears in a panel: which tool, which arguments, the result. Nothing happens invisibly.
5. **It understands the system.** It can read the kernel log, the process list and a crashed program's last words (`lastwords` already exists). "Why did that crash?" gets an answer from real data.
6. **It sees the screen.** The window server knows the text in every window, so "what does this error mean?" works on whatever is shown, without copy and paste.
7. **Events and conditions.** "When this finishes, turn off", "tell me when the download is done", "every morning, play the news". Triggers on a process ending, a file changing or a time, handled by the OS rather than by polling.
8. **Routines become scripts.** When I teach it something repeatable, it writes a plain `sh` script I can read, edit and run without the model. The AI helps create automation instead of being needed every time.
9. **Memory I control.** What it remembers about me lives in a plain text file I can read and edit. No hidden profile.
10. **Search by meaning.** Files are indexed by an embedding model, and the index is updated whenever fs's change stamps say a file changed. "Find the note about the bootloader bug" works without knowing the file's name.
11. **Offline by default, cloud optional.** The local engine is the default. A remote backend (a bigger cloud model) uses the same protocol, but needs a network capability I grant on purpose, and it shows when data leaves the machine.
12. **It shares the machine politely.** The model is unloaded when idle, runs at low priority on spare cores, and pauses when I'm gaming or compiling.

## What each of my examples needs

| I say | What TheTaaJ needs | Exists today? |
|---|---|---|
| "Go to that directory, check the file sizes" | the model, the assistant, file tools | fs yes; the rest is new |
| "When this is done, turn off" | wait for a job, then `power` | power and job waiting yes |
| "Explain the crash log" | the model reading a log | logs and `lastwords` yes |
| "Create a file and write what I say" | speech-to-text + file tools | speech: new |
| Answers spoken aloud | text-to-speech + sound output | no sound driver yet |
| "Play that song" | sound driver, audio service, MP3/FLAC decoders | new |
| "Uninstall that program" | knowing what's installed: a package manager | new |
| "Check the price on that website" | DNS, TLS (HTTPS), HTTP, HTML → text | TCP yes; the rest new. Pages that only show prices through JavaScript would need a browser engine |

## Where TheTaaJ stands today (2026-11-25)

| Needed for a model | Today |
|---|---|
| AVX/AVX2/FMA/VNNI in programs (2–4× faster maths) | The kernel saves only SSE state (`fxsave`): XSAVE is needed |
| Threads inside one program, on all cores | Processes are single-threaded |
| Memory for weights (1–3 GB) | 128 MiB per memory region (`abi::MEMORY_MAX`) |
| Files for weights | 256 MiB at most per TaajFS file |

## Model size

Using tools reliably over several steps needs roughly **1.5–4 billion parameters**. Candidates: **Qwen3-1.7B** or **Qwen3-4B** (Apache 2.0, trained for tool calling). At 4 bits they take about 1.1 GB and 2.5 GB, and give roughly 10–30 words a second on a desktop CPU with AVX2. Speech-to-text (Whisper tiny or base, 39–74M parameters) and text-to-speech (a Piper-style voice) run on the same engine.

Small local models make mistakes, so the confirmation step is not optional: it is what makes the assistant safe to use every day.

## A first draft of the eras

- **Era 8 · AI engine:**
  - XSAVE/AVX2 and threads inside a process;
  - our own tensor crate and transformer (stories15M first, checked token by token against the host);
  - GGUF, 4- and 8-bit weights, the tokenizer;
  - big files, and weights mapped into memory;
  - then Qwen3 chatting on TheTaaJ;
  - model manifests, and `model list/use/test/rollback`.
- **Era 9 · Assistant:**
  - the `infer` service and the agent loop;
  - tools published by services; task-scoped capabilities; confirmation; the activity log; undo snapshots;
  - "do X, then turn off";
  - a package manager, for programs and models.
- **Era 10 · Sound:** an Intel HD Audio driver, an audio service, WAV/MP3/FLAC, the microphone. "Play that song."
- **Era 11 · Voice:** Whisper speech-to-text, text-to-speech, a push-to-talk key, spoken answers.
- **Era 12 · Web:** DNS, TLS 1.3 (with its cryptography), HTTP, HTML to text. "Check the price", for pages that work without JavaScript.

Each era stays testable in QEMU: fixed prompts with greedy decoding, the tool calls compared exactly, sound captured to a file, voice fed from a recorded file.

## Open questions

1. Is the order of the eras right? Should the web come before sound?
2. My real PC's CPU and RAM: they decide whether Qwen3-4B is realistic or 1.7B is the target.
3. Our own inference engine (best for learning; llama.cpp on the host as the reference it must match), or a port of llama.cpp (quicker to get going)?
4. Which of the 12 ideas matter most, and which come first?
5. A wake word ("hey ...") or only a push-to-talk key? A wake word means the microphone is always listening.
6. What should the assistant be called?
