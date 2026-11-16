# LLM連携と安全性

## Providerとの境界

Claude連携は任意です。llm extraを導入し、ANTHROPIC_API_KEYをGit外に設定します。SHELTERSIM_LLM_MODEを省略するとauto modeになり、keyが設定されている場合、対応workflowがProviderを呼ぶことがあります。すべてローカルで行う場合はSHELTERSIM_LLM_MODE=offline、またはcommandの--offlineを指定します。同梱.env.exampleは`record`と`stop`で、API接続失敗時のoffline fallbackを防ぎます。

画面（`/`）の利用者向けデモはAPI接続を必須とし、`SHELTERSIM_LLM_MODE=record`と`SHELTERSIM_LLM_ERROR_POLICY=stop`で起動します。key不足やAPIエラーをoffline動作へ黙って切り替えず、処理失敗として表示します。オフラインmodeは自動テストなど、コマンドで明示した場合だけに使用します。

Anthropicへ接続するのはsrc/sheltersim/llm/anthropic_adapter.pyだけです。simulation coreにProvider依存はありません。requestはrouter、型付きtask schema、呼出し・token・同時実行・費用上限、usage ledger、cache、response tapeを通ります。

## 対応する段階

- 準備段階では合成Scenarioやprofile案を作れます。結果はpreviewであり、利用者が確認して同じhashを明示承認します。
- event解釈では導入済みprimitiveからEffectProgramを作成できます。previewまたは適用前に、現在Branch、対象ID、導入Policy、範囲、時系列に照らして検証します。
- runtimeでは設定済みのtriggerで少数の型付き提案を依頼する場合があります。毎tick・全員についてProviderを呼ぶ設計ではありません。
  - 支援要請の優先度判定（support-priority-v2）は、stress閾値と1日あたりの上限の範囲でだけ問い合わせます（model-validation.md参照）。以前のv1は依頼ごとに問い合わせていたため、既定の呼出し上限を開始直後に使い切り、social提案まで届きませんでした。v1の記録を含む履歴はv1のまま再生します。
  - v2のpromptはrationaleを80文字以内の短い句にするよう明示します。schemaの上限（160文字）を超えた応答は不正形式としてfallbackになります。
- reviewは保存Runの証拠を要約します。結果質問は記録された事実を根拠にするread-only操作で、ScenarioやBranchを変更できません。

利用可能な介入operatorはeffect-program.mdを参照してください。未導入機構が必要な要求は拒否するか、制限を示した近似として扱います。model responseはoperatorを追加したり、KPIを直接変更したり、Python/JavaScriptを実行したり、承認を迂回したりできません。

## 再現性とエラー

構造化responseをparseし、検証します。Provider error、不正形式、rate limit、offline mode、budget超過では、定義済みfallbackを使うか、利用不可を返します。選択された動作と利用量を記録します。

検証器は、Pydanticが生成するschemaで使う構文（`$ref`/`$defs`、`oneOf`、`const`、schemaとして与えた`additionalProperties`、数値・文字列の上下限、`pattern`）を扱います。以前は`$ref`を解決できず、Webの自由文読み取り（region-scenario-extraction-v1）は応答の内容にかかわらず必ず不合格になっていました。

### 分析用の予備枠

runtimeの呼出しは、Run（CLIでは1回のrun）の利用枠の上限から3回を残したところで止まります（`ANALYSIS_RESERVED_CALLS`）。残した3回は、終了後のreportと結果質問のための予備です。Projectでは、runtime・report・結果質問・介入の変換がすべて同じRunの利用枠を共有しています。この予備がないと、長いRunではruntimeが上限を使い切り、reportが生成されませんでした。

- 予備を超えたruntimeの依頼は`runtime_call_share_exhausted`としてfallbackし、fallback集計と警告に表示されます。
- 上限が小さい利用枠では、予備は「上限−1」回までです。上限1回の利用枠には予備がありません。
- 予備は呼出し回数だけを確保し、予算額は確保しません。

### 出典候補・report・結果質問の扱い

- 地域情報の公式ドメイン検索は、応答本文の引用に加えて、web searchが返した検索結果そのもの（URLとタイトル）を出典候補にします。JSONだけを返す応答には引用がほとんど付かず、以前は検索が成功しても候補が常に0件でした。候補は許可ドメイン（go.jp、lg.jp）に限り、重複を除いて最大25件です。ページ本文は取得・照合しません。
- Project Runの完了時は、reportを書き込んでから（生成できなかった場合はその旨を書き込んでから）Runを`completed`にします。画面は`running`から状態が変わった時点で一度だけreportを取得するためです。
- 利用記録は、同じ依頼（同じ入力・状態・prompt版）に二度課金しません。Webの下書きプレビューとProject Runの分析（report・結果質問）は、採用された応答を`usage-tapes/<draft>.json`と`runs/<run>/analysis-tape.json`に保存し、同じ依頼にはAPIを呼ばずに保存済みの応答を使います。以前は応答を保存していなかったため、同じ依頼を繰り返すと重複として拒否され、ruleの結果になっていました。保存するのは応答だけで、promptは保存しません。連絡先や秘密情報に見える文字列を含む応答は、保存せずにその場限りで使います。介入の文章の読み取り（compiler）の応答は、`runs/<run>/compiler-tape.json`に分けて保存します（#463）。
- 結果質問では、各根拠の値が記録値の完全な写しである必要があります。数値はJSONの値として、文字列はJSON形式（引用符付き）でもそのままでも受け付けます。丸めた値、型を変えた値、言い換えた文字列は拒否します。

### thinkingと出力上限

runtime以外のstage（準備、介入、review）が使うOpusは、thinkingを無効にできません。thinkingのtokenはAPIの`max_tokens`に含まれます。そのためrouterは、各taskの回答の大きさ（`max_output_tokens`、設定の`SHELTERSIM_LLM_MAX_OUTPUT_TOKENS`で上限）に、thinking用の余裕として4,096 tokenを加えて要求し、その分の費用も予約します。準備stageでは`effort: low`を指定し、thinkingを短くします。runtimeのHaikuは従来どおりthinkingを無効にします。

Opus呼出し1回の予約額は、既定設定でtaskの回答上限に応じておよそ$0.2〜0.4になりました（review約$0.26、シナリオ下書き最大約$0.38）。介入compilerは入力上限を64 KiBで予約するため、約$0.69です（以前は約$0.30）。実際の費用は送信量に応じて精算され、予約額よりずっと小さくなります。runの予算（`SHELTERSIM_LLM_BUDGET_USD`）がこれより小さいと、Opusの呼出しは`budget_exceeded`としてfallbackします。100人規模のシナリオ下書きは、回答が既定の出力上限（8,192）を超える見込みです。その場合は`SHELTERSIM_LLM_MAX_OUTPUT_TOKENS`を上げてください。

web searchのようなserver toolを使う呼出しでは、APIが検索の前後でモデルを複数回実行し、`usage.output_tokens`はその合計になります。予約は「検索回数＋1」回分の`max_tokens`で計算します。実際の使用量が予約を超えると、利用記録は安全のため凍結（frozen）され、以後その記録を共有するすべての有料呼出しが拒否されます。

凍結の原因を確認したら、次のコマンドで解除できます。対象は、PC全体で共有する累積上限の記録（task-global-v1）、CLIの利用枠（cli-…）、legacy demoの利用枠（legacy_demo）です。

~~~shell
sheltersim usage --unfreeze task-global-v1 --confirm task-global-v1 --reason "確認した理由"
~~~

- 解除で変わるのは凍結フラグだけで、記録済みの利用量は残ります。未決着（pending）の予約が残っている間は解除できません。
- 解除した日時・理由・その時点のreceipt数は、利用記録とは別の監査ファイル（usage-audit/）に残ります。usage summaryの`unfreeze_events`に表示されます。
- 利用記録の形式はv1のままです。PC全体で共有する記録なので、古い版の避難所IF！からも引き続き読めます。
- Project Runの利用枠は、このコマンドの対象外です。

### 入力の大きさの上限

routerは送信文のUTF-8バイト数を`max_input_tokens`（`SHELTERSIM_LLM_MAX_INPUT_TOKENS`、既定16,384）と比べ、超える依頼はAPIを呼ばずに`input_token_limit`としてfallbackします。バイト数はtoken数より多めの見積もりです。

文脈が大きくなるtaskは、task側で入力上限（`input_byte_limit`）を宣言できます。routerは設定値とtaskの宣言値の大きい方を使い、呼出し前の予算確認もその上限で予約します。宣言値は応答を変えないので、cache keyとresponse tapeには影響しません。

- 介入の自然言語指示（compiler）は64 KiBを宣言します。生成Scenarioでは導入済みprimitiveの説明だけで約13 KBあり、以前はScenarioの人数によらず16,384バイトを超えていました。その結果、Opusを一度も呼ばずにruleの結果になっていました。64 KiBは約1,500人までの文脈に相当します。
- それ以上の人数でも上限を超えると、これまでどおりAPIを呼ばずにfallbackし、その理由を結果に示します。
- usage ledgerの予約は、従来どおり実際の送信バイト数（入力とschema）から計算します。

### 介入compilerのfallback理由

介入の変換がruleの結果になったときは、Opusを使わなかった理由を日本語の固定文で`limitations`に入れます。対象はofflineで変換できた案、聞き返し、変換できなかった結果のすべてです。末尾の理由コードは`usage.error_kind`、またはrouterのoutcomeと同じです（例：`offline`、`input_token_limit`、`max_api_calls`、`budget_exceeded`、`rate_limited`、`invalid_output`）。model出力本文やprovider errorの本文は含めません。

`SHELTERSIM_LLM_ERROR_POLICY=stop`のときは、offline mode以外でruleの結果を返さず、同じ理由文で失敗します。Project APIのintervention-previewは409と`code: intervention_compiler_unavailable`、legacy demoの`/api/v1/nl/interpret`は409を返します。以前はstopでもcompilerだけがerrorを捕まえ、ruleの結果を返していました。明示したoffline modeでは、stopでもruleの結果を返します。

### fallbackの可視化

fallbackしたtaskはruleの既定値で進むため、その分の結果はofflineと同じになります。API modeのRunが実際にはClaudeの応答を使っていない状態を見落とさないよう、呼出し記録からoutcome集計を作ります。集計はsrc/sheltersim/llm/outcomes.pyの1関数に統一し、model出力本文は読みません。

| status | 意味 |
|---|---|
| not_requested | LLM taskを発行していません |
| offline_rules | offline modeで、全taskが設計どおりruleを使いました |
| all_accepted | 依頼した全taskが検証済みのProvider応答またはtapeを使いました |
| degraded | 一部のtaskがrule fallbackになりました |
| all_fallback | Provider応答を使えたtaskがありません |

fallbackの理由はoutcome別（max_api_calls、shared_allowance_unavailable、invalid_outputなど）、error_kind別、prompt版別に数えます。集計の表示先は次のとおりです。

- sheltersim runの出力JSONのllm_status（RunのLLMの利用記録〔`GET …/runs/{run}/usage`〕の`outcomes`。offlineのRunは`not_requested`）。degradedまたはall_fallbackのときはstderrに1行の警告を出します。終了codeは0のままです。
- Project Branchは GET /api/v1/projects/{project}/runs/{run}/branches/{branch}/llm-status。LLMで計算したRun（`runs/{run}/llm.json`のmodeが`offline`でないもの）は、保存したRunのLLMのテープ（形式v6の`llm_tape`の部品）の項目を、読み取り時に同じ規則で集計します（#463）。テープにはerror_kindがないので、`fallback_by_error_kind`はoutcome別と同じ数です。forkしたchild Branchの値には、fork前に親から引き継いだ項目も含みます。LLMなしのRunとブラウザー版は、何も問い合わせていない形（`mode: "offline"`、`not_requested`）を返します。

受理したresponse tapeはstate/input hash、model、prompt、schema版に結び付きます。tickの中の問い合わせのテープは、保存したRunの`llm_tape`の部品に保存します。過去seekは記録結果を使い、暗黙にProviderを呼びません。Branch stateが変わった場合、異なるinputにresponseを再利用しません。

## tickの中の問い合わせを中核で行う（#428）

tickの中でLLMに問い合わせる規則（支援要請の優先度`_prioritize_support_request`と、社会の方針`EventTriggeredSocialPolicy`）は、Rustの中核（`native/core/src/rules/llm_runtime.rs`）にあります（Pythonの規則は#430で消しました）。中核は、計算（`PlainRun`）の中で、この規則を自分で動かします。外とは`LlmLink::complete(task) -> reply`という1つの口（`native/core/src/llm.rs`）だけでつながります。ローカル版・VM版の計算は#427から`PlainRun`で行い、APIキーのあるサーバーでは、Runを作ったときのLLMのmode（`runs/{run}/llm.json`）で`RouterLink`を口にします（`web/worker_process.py`の`runtime_link`。modeが今の設定と違えば計算を断ります）。

- 口を通るのは、作業（`task`の名前、stage、プロンプトの版、schemaの版、入力の文、状態のhash、出力のschema、代替の文、出力の上限）と答え（文と、PythonのLLMCallRecordの結果・費用・token）のJSONだけです。プロンプトを作り、答えをschemaで確かめ、世界を書き換えるのは中核です。schemaに合わない答えは、口が何を返しても代替（`invalid_output`）になります。
- プロンプトは、#428で移したときのPythonの規則と同じ正規のJSONで、bytesも同じでした。状態のhashは中核の世界のdigestです。そのため、Pythonの経路で記録したresponse tape（PythonのCheckpointの中のもの）は再生できません。結果の世界は、ゲートの`llm_tape`のScenario（台本のProvider）が固定します。
- ローカル版・VM版では、`sheltersim.llm.link.RouterLink`がPythonの`LLMRouter`を口につなぎます（`PlainRun.set_llm(RouterLink(router))`）。予算・呼出し上限・response tape・利用記録のobserverは、routerのものがそのまま働きます。routerが止める設定（`LLMErrorPolicy.STOP`）の誤りや口が投げた例外は、tickを止め、`advance`の`RuntimeError`の原因（`__cause__`）になります。
- APIキーがない（offlineの）口には、中核は何も問い合わせず、世界も変えません。結果はLLMのないRunと同じです。社会の方針の判断の記録（`record_kind`が`dialogue`の行）は、保存したRunから外した後（#478）、中核も書かなくなりました（#556）。
- 問い合わせごとに、LLMのテープの項目（作業、版、入力と状態のhash、答えの文と結果・費用）を1つ残します（`PlainRun.take_tape()`）。保存の形式v6では`llm_tape`の部品に入ります。プロンプトとモデルの自由な文（優先度の`rationale`）は残しません。
- 保存した点の間の分を開くときは、前の保存した点に戻し、その点の件数より後のテープの項目で計算し直します（`PlainRun.set_llm_replay`、中核の`llm::Replay`）。順が違う問い合わせや、テープより先の問い合わせはtickを止めます。過去の計算でProviderを呼ぶことはありません。続きを計算するときは、テープの後に渡した口へ進みます。
- 社会の方針の上限（`SocialRuntimeLimits`、`SupportPriorityLimits`）の値と出典は、中核の`llm::Limits`にあります（ASM-EVENT-TRIGGERED-SOCIAL-LIMITS-V1、ASM-SUPPORT-PRIORITY-LIMITS-V1）。テストだけが`set_llm`の`limits`で変えます。
- ブラウザー版の中継（#287）も、同じ作業と答えのJSONで口を作れます。まだつないでいません（tickの中から呼ぶには同期の呼び出しが要るためです）。

## 結果の分析を中核で行う（#463）

結果への質問の回答、計算の終わりのレビュー、介入の文章の読み取り、PDFレポートの考察も、tickの中の問い合わせと同じ口（`LlmLink::complete(task) -> reply`）を使います。証拠の組み立て、作業（プロンプト・schema）、答えの確かめは中核（`native/core/src/answers/`）にあり、外は作業を口へ渡して答えを返すだけです。#427より前のPythonの世界や履歴の索引から証拠を作る処理（`web/results_chat.py`など）は、#427・#430で消しました。

- 2つの手順：中核が作業のJSONを作り（`Asking.task()`、`Reviewing.task()`、`Compiling.task()`、`PdfAnalysis.task()`）、外が口に渡して答えのJSONを返し、中核が答えを読みます（`Asking.answer(reply, mode, turn_id)`、`Reviewing.report(reply)`、`Compiling.read(reply)`、`PdfAnalysis.read(reply)`。介入の読み取りは、直してもらう作業を返すことがあります）。口が答えを後で返す場合（ブラウザーの`fetch`）も同じ作業を使えるよう、口を持ったまま呼ぶ形にはしていません。共通の部分は`answers/asking.rs`です。
- 答えは信頼しない入力です。中核は、まず作業のschemaで読み（`llm::check_answer`。Pythonの`validate_task_output`と同じschemaの範囲：配列とその要素・長さ、`$ref`、`anyOf`、`oneOf`、`const`、数の上下限、型のある`additionalProperties`、文字列の`pattern`）、合わない答えは代替（`invalid_output`）にします。そのあと、機能ごとに証拠と照らします。
- 結果への質問（`answers::questions`）：証拠は、Branchのその分のframe（KPI、選んだ人、最近の記録イベント）、比べるBranchがあれば比較の答え（`Comparison::answer`）、Runの`evidence.json`（モデルとその解釈）、Scenarioの出典、同じ会話の最近の質問と回答（保存したRunの`conversations`）です。事実（claims）は、引用した値が記録値の完全な写しのときだけ採り、1つでも違えば事実をすべて捨てます。説明（explanations）は、数字や人・IDの名前を含まず、引用が記録と合うものだけを「仮説」「検討案」「架空の合成会話」と明記して出します。何も裏付けられないときは「提示された記録だけでは回答を裏付けられません。」と答え、質問が1つの項目を名指ししていれば、その記録値をシステムが直接引用します。作業名は`result_question`、プロンプトの版は`read-only-results-chat-v2-grounded-explanations`です。回答の`usage`には、routerの`error_kind`と予算の残り（`budget_remaining_usd`）は入りません（口の答えにないため）。
- 計算の終わりのレビュー（`answers::report`）：LLMで計算したRunのBranchが終わったとき、計算のworkerが、終わりの分のRunから証拠（最後のKPI、重要なイベント、Scenarioの仮定のID）と作業（`run_review`、`evidence-review-v1`）を作り、Runのrouterに問い合わせ、答えの符号を証拠と照らしてreportを保存してから、Branchを`completed`にします（`web/worker_process.py`の`review`）。合えば`validated`、合わなければ`rejected`（理由つき）、代替なら`rule_fallback`です。markdownは証拠の値と決まった語だけで作り、モデルの文は入りません。比べるBranchのない証拠なので、増減（`increase`など）を答えた符号は`rejected`になります。問い合わせに失敗してもBranchは`completed`のままで、reportは最初に読まれたときにLLMなしで作ります。LLMなしのRun（画面のofflineのRun、offlineの実験）は、今までどおり最初に読まれたときに作ります。
- 介入の文章の読み取り（`interpret::Compiling`、`answers::interventions::PreviewPlan::compiling`）：APIキーのあるサーバーでは、#463より前のPythonの`compile_natural_language`と同じ作業（`intervention_compiler`、介入のstage、`geometry-population-effect-compiler-v2`、入力の上限64 KiB）で読みます。LLMに示す作用は、中核が確かめて予定できる6つ（`facility_outage`、`add_staff`、`add_toilet`、`water_outage`、`add_evacuees`、`depart_evacuees`、`program::supports`）のうち、その分岐の方策と対象で使えるものだけです。ほかの作用の案は、プレビューも適用もできないためです。答えは、状態ごとの決まり、作用の対象・時刻・上限、中核のEffectProgramの契約で確かめ、断られた案は誤りを添えて1回だけ直してもらいます（`intervention_repair`、`h07-opus-effect-compiler-v1-repair1`）。受け付けた案がないときは規則で読み、その理由を`limitations`に書きます。routerが止める設定（`SHELTERSIM_LLM_ERROR_POLICY=stop`）では、規則に替えず409 `intervention_compiler_unavailable`で断ります。Opusが区域を聞き返し、規則がシナリオのデータから区域を決められるときは、規則の案を使います（#35）。採用した応答はRunの`compiler-tape.json`に保存します。
- PDFレポートの考察（`answers::pdf`、`PdfPlan`・`PdfAnalysis`）：レポートのデータ（条件、各BranchのKPI・時系列・問題の継続・記録したイベントと介入、B−Aの差）と、LLMが引用できる根拠（facts。利用者の名前・場所・ID・自由な文は送らない別名だけ）を中核が作り、作業（`pdf_analysis`、`pdf-analysis-ja-v1`、Opus）を問い合わせます。答えはOpusであること、schema、各段落の根拠ID、数値・引用・因果断定の表現、仮説と提案の文末で確かめ、通らなければ409で断ってレポートを作りません。道の答えはデータ・根拠・確かめた分析のJSONで、画面が印刷用ページに並べます（#470、[pdf-report-export.md](pdf-report-export.md)）。保存はPythonです。
- ローカル版・VM版の口は、Runの利用枠で数える`RouterLink`です（`sheltersim.llm.link.analysis_link`）。採用した応答はRunの`analysis-tape.json`に保存するので、replay modeでは、同じ証拠への同じ問い合わせにProviderを呼ばずに同じ答えを返します。APIキーがない（effective modeがofflineの）サーバーは口を作らず、今までと同じbytesの答え（ブラウザー版と同じ）を返します。
- ブラウザー版は、中核の作業のJSONをそのまま中継に送ります（#470、下の「ブラウザー版の中継」の「中核の作業」）。中継は作業を中核の作業目録と照らし、プロンプトはPythonのrouterと同じに作ります。ブラウザーは、結果への質問と介入の文章の読み取りを中継に頼みます（[project-persistence.md](project-persistence.md#ブラウザーの比較と質問307308)）。PDFの考察は#470の続きのPull Requestでつなぎます。

## ブラウザー版の中継（#419）

ブラウザー版（静的なサイト）にはAPIキーを置けないため、同じCloudflareのWorkerのscript（`deploy/workers/relay.mjs`）が中継します。決めたことは#287にあります。開発と公開の手順は`docs/development.md`の「LLMの中継」です。

- 道：`POST /llm/v1/session`（合言葉を確かめ、相手の名前と今日の残りを返す）と、`POST /llm/v1/tasks/{作業}`（`{"input": …}`）。合言葉は`Authorization: Bearer`で送ります。同じoriginからだけ受け、256 KiBより大きい本文は断ります（介入の作業の入力64 KiBとschemaが入る大きさ）。断るときは`{"error", "code"}`で、`error`は画面にそのまま出せる日本語です。
- プロンプトは中継の中で作業ごとに作ります（`deploy/workers/relay_tasks.mjs`）。画面が送るのはデータだけです。答えはモデルの文をそのまま返し、作業名、プロンプトの版、モデル、tokenと費用を添えます。画面は、Pythonと同じく、信頼しない入力として型と意味を確かめてから使い、ScenarioやBranchを変える前に利用者の承認を求めます。
- 費用：サイト全体で、日本時間の1日に$5までです。PythonのLLMRouterと同じく、呼ぶ前に最大の費用（送る文のUTF-8のbytes＋256を1時間のcache書き込みの値段で、`max_tokens`を出力の値段で）を予約し、答えのusageで精算します。値段はPythonの`_OFFICIAL_MODEL_PRICE_FLOORS`と同じ表を`relay.mjs`の`PRICES`に持ち、`tests/unit/llm/test_relay.py`が同じであることを確かめます。Opusは、Pythonと同じく、thinkingの分として4,096 tokenを足して求めます。APIが誤りを返したときは0で、答えが来なかったときは予約した額で精算します。予約と精算は、サイトに1つのDurable Object（SQLite）で行います。
- 合言葉は配る相手ごとに別で、中継はそのSHA-256と相手の名前だけを持ちます（secret `LLM_PASSPHRASES`）。比べるときは、全部の項目を最後まで比べます。相手ごとの回数・費用・tokenを記録し、ログにも合言葉、APIキー、送った文は出しません。
- 合言葉はサイトの管理の頁`/admin`（#433）でも発行・停止できます。管理の道（`/llm/v1/admin/*`）はWorkerのsecret `LLM_ADMIN_KEY`（管理用の鍵）を`Authorization: Bearer`で求めます。発行した合言葉は一度だけ返し、中継はそのSHA-256と相手の名前をDurable Objectの`recipients`の表に残します。頁は相手ごとの今日と累計の回数・費用を出します。鍵の誤りは合言葉の誤りとは別に数え、続けば止めます。
- 標準のpytestとCIは本物のAPIを呼びません。中継のテストは、Anthropicへの接続を決めた答えに差し替えます。
- 作業`scenario_text`（#421）：シナリオ作成の文を、Pythonの`parse_scenario_text`がrouterで読むのと同じ作業（`region-scenario-extraction-v1`、PREPAREのOpus、`effort: low`、答えは1,600 token、Pythonと同じsystem promptとschema）で読みます。作業の中身は`native/core/src/scenario/scenario_text_task.json`に1つだけ置き、中継とブラウザーの中核の両方が読みます。このファイルは`native/core/tools/scenario_text_task.py`がPythonから書き、`tests/unit/llm/test_region_prepare.py`が古くなっていないことを、`tests/unit/llm/test_relay.py`が中継の送るプロンプトがPythonと同じであることを確かめます。
- 答えは、ブラウザーの中核（`sheltersim_core::scenario::text::parse_answer`）がPythonと同じく読みます。空・JSONでない・schemaに合わない答えはルールの代替（`rules_fallback`）、`_parse_llm_contract`が断る答えはルールの値だけ（`rules`）、使えた答えは`prepare_llm`です。答えに、ブラウザー版がまだ計算しない条件（人口の条件、避難の選び方、施設の区画、区画の面積の割合、運用の予定など）があるときは、答えを使わずにAPIキーなしと同じく読み、そのことを`warnings`の先頭に書きます（Pythonはこの答えを使います）。中継が答えないとき（合言葉、その日の上限、APIへの接続）も、APIキーなしと同じく読み、理由を`warnings`の先頭に書きます。`native/core/tests/scenario_text.rs`が、答えの例（`native/core/tests/data/scenario_text_answers.json`）でPythonと同じ結果になることを確かめます。
- 中核の作業（#470）：結果への質問（`result_question`）、介入の文章の読み取り（`intervention_compiler`）とその修正（`intervention_repair`）、PDFレポートの考察（`pdf_analysis`）。ブラウザーは中核が作った作業のJSON（`llm::Task`の形）を、そのまま`input`に入れて送ります。証拠の材料はブラウザーのRunにあって大きいため、中継で入力を作り直すことはしません。代わりに中継は、中核から書き出した作業目録`native/core/src/answers/relay_tasks.json`（`sheltersim_core::answers::relay`）と照らして受けます。照らすのは、作業名、stage、プロンプトとschemaの版、出力の上限、入力の上限（`max_input_bytes`。routerと同じく、作業の上限と16,384 bytesの大きいほう）、固定のschema（質問とPDF）です。入力の文はJSONのobjectであること、作業が持ってくるschema（介入のもの。シナリオの対象で変わる）は32 KiBまでのobjectであることも確かめます。合わない作業は400 `llm_task_input_invalid`で、APIを呼びません（ページが古いときは読み込み直しを求めます）。呼び方はPythonのrouterと同じです。stageのモデル（どちらもOpus）、Pythonの`_system_prompt`と同じsystem prompt（schemaの小数と日本語も`json.dumps`と同じ書き方）、入力の文を1つのuser message、`effort`なし、Opusのthinkingの分の4,096 tokenです。`tests/unit/llm/test_relay.py`が目録の作業ごとに、`tests/integration/test_llm_answers.py`と`test_pdf_report.py`がProject APIで中核が実際に作った作業で、中継の送る要求がrouterの要求と同じであることを確かめます。目録は`native/core/tests/llm_answers.rs`が、古くなっていないこと、中核の作る作業が目録に合うことを確かめます（書き直しは`SHELTERSIM_WRITE_RELAY_TASKS=1 cargo test -p sheltersim_core --test core llm_answers::`）。費用の上限は、サイト全体の1日$5だけです。作業ごと・相手ごとの上限は設けません（#470の決定）。
- プレビューの`llm_usage`はPythonのrecord modeと同じ形（`mode: "record"`、呼んだ回数、費用）です。呼んだ記録（モデル、プロンプトの版、token、費用、結果）は、準備の勘定（`preparation.json`の`llm_calls`）に残り、承認でRunに結び付きます。合言葉はどこにも保存しません。

## プライバシーと安全性

- key、.env、実在避難者の記録、機微な生prompt、未確認の施設図面をcommitしないでください。
- Scenario文章、取込データ、Provider出力を信頼しない入力として扱います。
- 匿名IDを使い、対応taskに必要な項目だけを送信します。
- 該当validatorがある場合、連絡先のような情報をredactまたは拒否します。browser stateやProject recordへProvider keyを保存しません。
- 結果Q&Aはread-onlyとし、保存証拠に結び付けます。
- 標準pytest、lint、CIからlive Providerを呼びません。

API keyはprocess環境変数から読みます。.env.exampleは設定名の参考であり、dotenv loaderではありません。Provider requestはrouterごとの予算と、ローカルProject・process間で共有する既定50ドルのusage ledgerに対して予約します。端末の共有上限を明示的に増やす場合は`usage --increase-budget task-global-v1 --new-budget-usd <新しい合計額> --confirm task-global-v1 --reason "承認理由"`を使います。変更理由と旧・新上限は端末内の監査記録に残り、Git管理の既定上限には反映されません。この上限はソフトウェアの制限であり、利用承認ではありません。API callにはプロジェクトオーナーの都度の承認が必要です。別taskの予算や承認は引き継がれません。公開前に累積上限の実装を再検討してください。

## 利用記録とlive連携test

CLI usage commandと/api/v1/usage/runtimeは上限付きusage receiptを表示します。Project RunにもRun単位の会計情報と操作recordが保存されます。

CLIはprepareのたびに新しい利用枠（cli-prepare-…）を作り、その時点の設定を上限として固定します。runはProjectのRunを作るので、画面のRunと同じくRunの利用枠を使います（#450）。以前は全CLI実行が固定の cli-default を共有していました。一度使い切ると、それ以降はAPIを呼ばずに全taskがfallbackしていました。既存の cli-default は使われなくなるだけで、記録は残ります。Run間の合計は、既定50ドルまたは端末台帳の監査済み増額により制限されます。

live連携testは標準testとは別で、`tests/live/test_live_api.py`にあります。ANTHROPIC_API_KEYとSHELTERSIM_ALLOW_LIVE_API_TESTS=1の両方が必要で、pytest実行中にAPIキーを使えるのはこのファイルのテストだけです。設定の既定値は2 call、test budget 0.01ドルですが、OpusとSonnetの呼出しはthinking用の余裕を予約するため、この既定値ではOpusを1回も呼べません。そのため各testは、上限を自分で設定します。

- モデル別確認：3 call、0.15ドル。runtimeの呼び出しは分析用の枠（`ANALYSIS_RESERVED_CALLS`）を残す必要があるので、Haiku（runtime）を最初に、Opus・Sonnetをその後に呼びます。上限は実行ごとの口座（`live-task-qa-<乱数>`）に掛かり、全実行の合計は端末の共有上限（task-global-v1）が抑えます。この設定と順番が、2回続けても台帳の上限に収まることは、`tests/unit/llm/test_accounting.py`がAPIを呼ばずに確かめます（#148）。
- 公式ドメイン検索：2 call分の枠。モデル別確認と同じく実行ごとの口座（`live-source-search-stream-qa-<乱数>`）に掛かり、続けて実行しても予約できることを`tests/unit/llm/test_accounting.py`が確かめます（#152）。
- 機能の通し確認：各LLMの段階を今の道で1回ずつ通します。Scenarioの下書き（`generate_scenario_draft`、Opus）、中核のtickの中のruntimeの問い（`PlainRun.set_llm`と`RouterLink`、Haiku。全員のストレスを高くし、問いは1日に社会の判断1件と支援の優先度1件まで）と、そのLLMテープからの再計算（Providerを呼ばず、同じworld）、LLMつきで計算したProject Runのレビュー（Opus）、介入文の読み取り（`POST …/intervention-preview`、Sonnet。下書きだけで、承認するまでBranchは変わりません）、質問（`POST …/questions`、Opus）と、replayモードでの同じ質問（Runの分析テープから答え、Providerを呼ばない）です。利用枠は、準備とruntimeの口座（`live-feature-<乱数>`）とProject Runの口座に、それぞれ12 call・1ドルです。同じ手順（`tests/support/llm.py`の`live_feature_route`）を、`tests/integration/test_llm_answers.py`が偽のProviderでAPIを呼ばずに毎回実行します。手順が使う道がなくなったとき、ゲートを開く前に気づけます（#522）。

実際の費用は、予約額よりずっと小さくなります。実行前に実効上限を確認し、その時点でのオーナー承認を得てください。

`scripts/test-live-api.sh`は、2つの環境変数がそろっているかを確かめてから、`tests/live/test_live_api.py`を`-m live_api`で実行します。開発依存を入れた環境（`.venv`など）を有効にして使います。後ろに付けた引数はpytestに渡るので、`-k`で1件だけ実行したり、`--collect-only`でAPIを呼ばずに対象を確かめたりできます。

~~~shell
SHELTERSIM_ALLOW_LIVE_API_TESTS=1 scripts/test-live-api.sh -k opus_sonnet_and_haiku
SHELTERSIM_ALLOW_LIVE_API_TESTS=1 scripts/test-live-api.sh --collect-only -q
~~~offline testの実行方法と結果の見方はtesting.mdを参照してください。
