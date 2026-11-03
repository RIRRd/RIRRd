# AI disclosure and contributions

RIRRd is built with AI assistance, including code, tests, documentation, tooling
and the web interface. AI tools and coding agents are welcome to contribute.

AI authorship is not evidence that a change is correct, secure, compatible with
IRRd or ready for production. Contributors remain responsible for understanding
and reviewing their submissions, checking source provenance and licensing,
protecting secrets, and providing reproducible tests. Human review and verified
CI are required before changes reach protected main or a public registry release.
Do not label a generated test, passing subset or benchmark as proof of full parity.

AI-assisted contributions follow the same contribution process and MIT licensing
as other contributions. Explain meaningful design choices, cite official sources
when compatibility depends on them, and report test failures and limitations.
Never commit real credentials, private prompts containing sensitive information,
model-provider tokens, registry runtime data or production snapshots.

The application does not require an AI model, cloud inference service, Node
runtime or external analytics to run. The bundled dashboard uses local assets
and queries the local registry API. AI is part of the development process, not a
claim of runtime validation or automatic trust.
