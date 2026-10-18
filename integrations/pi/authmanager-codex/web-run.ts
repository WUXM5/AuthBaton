import type { ExtensionAPI, SessionEntry } from "@earendil-works/pi-coding-agent";
import { StringEnum } from "@earendil-works/pi-ai";
import { Text } from "@earendil-works/pi-tui";
import { Type } from "typebox";

// Codex's own web search for the models it runs in "Responses Lite" mode: no hosted search tool,
// but a client-run `web.run` that posts the model's commands to the Codex backend's standalone
// search. Its output stays in the transcript as the tool result, so later turns still have what
// was found.
//
// The tool description and command schema are adapted from OpenAI Codex rust-v0.163.0
// (codex-rs/ext/web-search, codex-rs/codex-api/src/search.rs), Copyright 2025 OpenAI, licensed under
// the Apache License, Version 2.0 (http://www.apache.org/licenses/LICENSE-2.0). Changed: the tool is
// named `web_run` instead of `web.run`.

const NAME = "web_run";

const DESCRIPTION = `Tool for accessing the internet.

---

## Examples of different commands available in this tool

Examples of different commands available in this tool:
* \`search_query\`: {"search_query": [{"q": "What is the capital of France?"}, {"q": "What is the capital of belgium?"}]}. Searches the internet for a given query (and optionally with a domain or recency filter)
* \`image_query\`: {"image_query":[{"q": "waterfalls"}]}.
* \`open\`: {"open": [{"ref_id": "turn0search0"}, {"ref_id": "https://www.openai.com", "lineno": 120}]}
* \`click\`: {"click": [{"ref_id": "turn0fetch3", "id": 17}]}
* \`find\`: {"find": [{"ref_id": "turn0fetch3", "pattern": "Annie Case"}]}
* \`screenshot\`: {"screenshot": [{"ref_id": "turn1view0", "pageno": 0}, {"ref_id": "turn1view0", "pageno": 3}]}
* \`finance\`: {"finance":[{"ticker":"AMD","type":"equity","market":"USA"}]}, {"finance":[{"ticker":"BTC","type":"crypto","market":""}]}
* \`weather\`: {"weather":[{"location":"San Francisco, CA"}]}
* \`sports\`: {"sports":[{"fn":"standings","league":"nfl"}, {"fn":"schedule","league":"nba","team":"GSW","date_from":"2025-02-24"}]}
* \`time\`: {"time":[{"utc_offset":"+03:00"}]}

---

## Usage hints
To use this tool efficiently:
* Use multiple commands and queries in one call to get more results faster; e.g. {"search_query": [{"q": "bitcoin news"}], "finance":[{"ticker":"BTC","type":"crypto","market":""}], "find": [{"ref_id": "turn0search0", "pattern": "Annie Case"}, {"ref_id": "turn0search1", "pattern": "John Smith"}]}
* Use "response_length" to control the number of results returned by this tool, omit it if you intend to pass "short" in
* Only write required parameters; do not write empty lists or nulls where they could be omitted.
* \`search_query\` must have length at most 4 in each call. If it has length > 3, response_length must be medium or long
* If you find yourself in a situation where you accidentally call the \`${NAME}\` tool, it's best just to send an empty query: {"search_query": [{"q": ""}]}.

---

## Decision boundary

If the user makes an explicit request to search the internet, find latest information, look up, etc (or to not do so), you must obey their request.
When you make an assumption, always consider whether it is temporally stable; i.e. whether there's even a small (>10%) chance it has changed. If it is unstable, you must verify with browsing the internet for verification.

<situations_where_you_must_browse_the_internet>
Below is a list of scenarios where browsing the internet MUST be used. PAY CLOSE ATTENTION: you MUST browse the internet in these cases. If you're unsure or on the fence, you MUST bias towards browsing the internet.
- The information could have changed recently: for example news; prices; laws; schedules; product specs; sports scores; economic indicators; political/public/company figures (e.g. the question relates to 'the president of country A' or 'the CEO of company B', which might change over time); rules; regulations; standards; software libraries that could be updated; exchange rates; recommendations (i.e., recommendations about various topics or things might be informed by what currently exists / is popular / is safe / is unsafe / is in the zeitgeist / etc.); and many many many more categories -- again, if you're on the fence, you MUST browse the internet!
  - For news queries, prioritize more recent events, ensuring you compare publish dates and the date that the event happened.
- The user is seeking recommendations that could lead them to spend substantial time or money -- researching products, restaurants, travel plans, etc.
- The user wants (or would benefit from) direct quotes, links, or precise source attribution.
- A specific page, paper, dataset, PDF, or site is referenced and you haven't been given its contents.
- You're unsure about a fact, the topic is niche or emerging, or you suspect there's at least a 10% chance you will incorrectly recall it
- High-stakes accuracy matters (medical, legal, financial guidance). For these you generally should search by default because this information is highly temporally unstable
- The user explicitly says to search, browse, verify, or look it up.
</situations_where_you_must_browse_the_internet>

---

## Citations

Results from \`${NAME}\` include internal reference IDs such as \`turn2search5\`. Use
those reference IDs only in calls to \`${NAME}\`; do not expose them in the final
response.

Cite sources in the final response using Markdown links:

- Cite a single source as \`[descriptive source title](https://example.com/page)\`.
- Cite multiple sources with separate Markdown links, for example
  \`[first source](https://example.com/one), [second source](https://example.com/two)\`.
- Link directly to the page that supports the claim. Do not link to search result
  pages or use bare URLs.

Formatting of citations:

- Place each citation as near as possible to the claim it supports, normally at
  the end of the sentence or paragraph and after punctuation.
- Do not place citations inside code fences.
- Do not put citations on a line by themselves or collect all citations at the
  end of the response.

If you browse the internet, cite statements supported by web sources. Each cited
source must directly support the associated claim. Prefer primary and
authoritative sources, and use sources from different domains when the response
benefits from multiple perspectives.

---

## Special cases
If these conflict with any other instructions, these should take precedence.

<special_cases>
- When the user asks for information about how to use OpenAI products, (ChatGPT, the OpenAI API, etc.), you should check the code in local env and only browse as fallback, when you browse restrict your sources to official OpenAI websites using the domains filter, unless otherwise requested.
- When using search to answer technical questions, you must only rely on primary sources (research papers, official documentation, etc.)
- Clearly indicate when you are making an inference from sources.
</special_cases>

---

## Word limits
Responses may not excessively quote or draw on a specific source. There are several limits here:
- **Limit on verbatim quotes:**
  - You may not quote more than 25 words verbatim from any single non-lyrical source, unless the source is reddit.
  - For song lyrics, verbatim quotes must be limited to at most 10 words.
  - Long quotes from reddit are allowed, as long as you indicate that those are direct quotes via a markdown blockquote starting with ">", copy verbatim, and link the source.
- **Word limits:**
  - Each webpage source in the sources has a word limit label formatted like "[wordlim N]", in which N is the maximum number of words in the whole response that are attributed to that source. If omitted, the word limit is 200 words.
  - Non-contiguous words derived from a given source must be counted to the word limit.
  - The summarization limit N is a maximum for each source.
  - When using multiple sources, their summarization limits add together. However, each article used must be relevant to the response.
- **Copyright compliance:**
  - You must avoid providing full articles, long verbatim passages, or extensive direct quotes due to copyright concerns.
  - If the user asked for a verbatim quote, the response should provide a short compliant excerpt and then answer with paraphrases and summaries.
  - Again, this limit does not apply to reddit content, as long as it's appropriately indicated that those are direct quotes and you link to the source.
`;

const query = Type.Object({
	q: Type.String({ description: "Search query." }),
	recency: Type.Optional(Type.Integer({ minimum: 0, description: "Whether to filter by recency, as a number of recent days." })),
	domains: Type.Optional(Type.Array(Type.String(), { description: "Whether to filter by a specific list of domains." })),
});
const list = <T extends Parameters<typeof Type.Array>[0]>(item: T, description: string) => Type.Optional(Type.Array(item, { description }));

const parameters = Type.Object({
	search_query: list(query, "Query the internet search engine for a given list of queries."),
	image_query: list(query, "Query the image search engine for a given list of queries."),
	open: list(Type.Object({
		ref_id: Type.String({ description: "Reference id or URL to open." }),
		lineno: Type.Optional(Type.Integer({ minimum: 0, description: "Line number to position the page at." })),
	}), "Open pages by reference id or URL."),
	click: list(Type.Object({
		ref_id: Type.String({ description: "Reference id containing the numbered link." }),
		id: Type.Integer({ minimum: 0, description: "Numbered link id to open." }),
	}), "Open links from previously opened pages."),
	find: list(Type.Object({
		ref_id: Type.String({ description: "Reference id or URL to search within." }),
		pattern: Type.String({ description: "Text pattern to find." }),
	}), "Find text patterns in pages."),
	screenshot: list(Type.Object({
		ref_id: Type.String({ description: "Reference id or URL to screenshot." }),
		pageno: Type.Integer({ minimum: 0, description: "Zero-indexed PDF page number." }),
	}), "Take screenshots of PDF pages."),
	finance: list(Type.Object({
		ticker: Type.String({ description: "Ticker symbol to look up." }),
		type: StringEnum(["equity", "fund", "crypto", "index"], { description: "Asset type to look up." }),
		market: Type.Optional(Type.String({ description: "ISO 3166-1 alpha-3 country code, \"OTC\", or \"\" for cryptocurrency." })),
	}), "Look up prices for the given stock symbols."),
	weather: list(Type.Object({
		location: Type.String({ description: "Location in \"Country, Area, City\" format." }),
		start: Type.Optional(Type.String({ description: "Start date in YYYY-MM-DD format. Defaults to today." })),
		duration: Type.Optional(Type.Integer({ minimum: 0, description: "Number of days to return. Defaults to 7." })),
	}), "Look up weather forecasts."),
	sports: list(Type.Object({
		tool: Type.Optional(StringEnum(["sports"], { description: "Tool name for sports requests." })),
		fn: StringEnum(["schedule", "standings"], { description: "Sports function to call." }),
		league: StringEnum(["nba", "wnba", "nfl", "nhl", "mlb", "epl", "ncaamb", "ncaawb", "ipl"], { description: "League to look up." }),
		team: Type.Optional(Type.String({ description: "Team to look up, using the common 3 or 4 letter alias used in broadcasts." })),
		opponent: Type.Optional(Type.String({ description: "Opponent to use with `team` when narrowing the lookup." })),
		date_from: Type.Optional(Type.String({ description: "Start date in YYYY-MM-DD format." })),
		date_to: Type.Optional(Type.String({ description: "End date in YYYY-MM-DD format." })),
		num_games: Type.Optional(Type.Integer({ minimum: 0, description: "Number of games to return." })),
		locale: Type.Optional(Type.String({ description: "Locale for the lookup." })),
	}), "Look up sports schedules and standings."),
	time: list(Type.Object({ utc_offset: Type.String({ description: "UTC offset formatted like \"+03:00\"." }) }), "Get time for the given UTC offsets."),
	response_length: Type.Optional(StringEnum(["short", "medium", "long"], { description: "Set the length of the response to be returned." })),
});

/** A result as the search endpoint lists it for display. */
type Result = { title?: string; url?: string };

// Codex sends the two latest user messages and up to 1k tokens of the assistant text between them.
function recentInput(entries: SessionEntry[]): unknown[] {
	const messages = entries.flatMap((entry) => entry.type === "message" && (entry.message.role === "user" || entry.message.role === "assistant") ? [entry.message] : []);
	const users = messages.flatMap((message, index) => message.role === "user" ? [index] : []);
	let budget = 2000;
	return messages.slice(users.at(-2) ?? users.at(-1) ?? messages.length).flatMap((message) => {
		const content = message.content as string | { type: string; text?: string }[];
		const text = typeof content === "string" ? content : content.flatMap((block) => block.type === "text" && block.text ? [block.text] : []).join("\n");
		if (message.role === "user") return text ? [{ type: "message", role: "user", content: [{ type: "input_text", text }] }] : [];
		const kept = text.slice(0, budget);
		budget -= kept.length;
		return kept ? [{ type: "message", role: "assistant", content: [{ type: "output_text", text: kept }] }] : [];
	});
}

/**
 * Registers `web_run`, active only while the model is one of `provides` (the AuthManager providers); `token` gives
 * the access token of that provider's account.
 */
export function registerWebRun(pi: ExtensionAPI, baseUrl: string, provides: (provider: string) => boolean, token: (provider: string, signal?: AbortSignal) => Promise<string>): void {
	pi.registerTool({
		name: NAME,
		label: NAME,
		description: DESCRIPTION,
		parameters,
		defaultActive: false,
		async execute(_toolCallId, params, signal, _onUpdate, ctx) {
			const model = ctx.model;
			if (!model || !provides(model.provider)) throw new Error(`${NAME} only works with AuthManager's Codex models`);
			const access = await token(model.provider, signal);
			const claims = JSON.parse(Buffer.from(access.split(".")[1], "base64url").toString("utf8"));
			const response = await fetch(`${baseUrl}/codex/alpha/search`, {
				method: "POST",
				signal,
				headers: { authorization: `Bearer ${access}`, "chatgpt-account-id": claims["https://api.openai.com/auth"].chatgpt_account_id, originator: "pi", "content-type": "application/json" },
				body: JSON.stringify({
					// The session id keeps reference ids such as turn0search1 valid across calls.
					id: ctx.sessionManager.getSessionId(),
					model: model.id,
					input: recentInput(ctx.sessionManager.getBranch()),
					commands: params,
					settings: { allowed_callers: ["direct"], external_web_access: true },
					max_output_tokens: 10000,
				}),
			});
			if (!response.ok) throw new Error(`${NAME} failed: HTTP ${response.status} ${(await response.text()).slice(0, 300)}`);
			const body = await response.json() as { output: string; results?: Result[] };
			return {
				content: [{ type: "text", text: body.output }],
				details: { results: (body.results ?? []).map(({ title, url }) => ({ title, url })) },
			};
		},
		renderCall(args, theme) {
			const parts = Object.entries(args).flatMap(([command, operations]) => !Array.isArray(operations) ? []
				: operations.map((operation: Record<string, unknown>) => command === "search_query" ? String(operation.q ?? "") : `${command} ${Object.values(operation).join(" ")}`));
			return new Text(`${theme.fg("toolTitle", theme.bold(NAME))} ${theme.fg("accent", parts.join(" · "))}`, 0, 0);
		},
		renderResult(result, { expanded, isPartial }, theme, context) {
			if (isPartial) return new Text(theme.fg("dim", "Searching..."), 0, 0);
			if (context.isError) return new Text(theme.fg("error", result.content.map((block) => block.type === "text" ? block.text : "").join("\n")), 0, 0);
			const results = (result.details as { results: Result[] }).results;
			if (!expanded) return new Text(theme.fg("dim", `${results.length} result${results.length === 1 ? "" : "s"}`), 0, 0);
			return new Text(results.map((item) => `${item.title ?? ""} ${theme.fg("dim", item.url ?? "")}`).join("\n"), 0, 0);
		},
	});
	// Other models get no tool they cannot use; pi's own tool choices stay as they are.
	const follow = (provider: string | undefined): void => {
		const active = pi.getActiveTools();
		const wanted = provider !== undefined && provides(provider);
		if (active.includes(NAME) !== wanted) pi.setActiveTools(wanted ? [...active, NAME] : active.filter((name) => name !== NAME));
	};
	pi.on("session_start", (_event, ctx) => { follow(ctx.model?.provider); });
	pi.on("model_select", (event) => { follow(event.model.provider); });
}
