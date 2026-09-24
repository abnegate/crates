# abnegate-llm

An OpenAI-compatible chat completions client, a provider abstraction that puts
several of them behind one handle, and one trait per generative modality for the
vendors that do not speak that API. `LlmClient` speaks the chat completions API,
streaming or not. `CompletionProvider` is the contract a source of completions
satisfies, and `Router` satisfies it over a set of providers, so a consumer never
learns whether it is talking to one model, an A/B split, or a fallback chain.
`modality` holds one trait per modality (text, image, audio, voice, video, 3D
model, embedding and transcription) with the vendor-native Anthropic, Gemini and
OpenAI clients behind their features; `cost` picks a model for a task under a
`CostStrategy`, `hardware` says what a machine can run locally, and `catalog`
browses the model catalogues those choices are made from.

## Features

- `anthropic`: the Anthropic messages client behind `TextProvider`.
- `google`: the Gemini client behind `TextProvider`.
- `openai`: the OpenAI client behind the text, image, embedding and transcription traits.
- `catalog`: browse the Ollama library, HuggingFace, GPT4All and OpenRouter catalogues through one `catalog::ModelProvider` trait.
- `download`: resumable GGUF downloads that only splice a resume onto the same upstream file and verify a SHA-256 when one is given.
- `testing`: `provider::testing::StubProvider`, a completion provider whose answers a test decides.

## Usage

```sh
cargo add abnegate-llm
```

```rust,no_run
use abnegate_llm::{Error, LlmClient, LlmConfig, Message};

async fn greet() -> Result<(), Error> {
    let client = LlmClient::new(LlmConfig::new("http://127.0.0.1:4000/v1", "qwen3", ""));

    let response = client.chat(&[Message::user("Say hello.")], None).await?;
    println!("{:?}", response.choices[0].message.content);
    Ok(())
}
```
