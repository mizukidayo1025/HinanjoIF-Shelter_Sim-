# CLI・データ契約・HTTP API

## 版管理されるデータ

正準input/output schemaはパッケージ内で実装し、テストで確認しています。主なrecordは次のとおりです。

| Record | 用途 |
|---|---|
| ScenarioSpec | 固定された人口、避難所、schedule、モデル設定 |
| ProfileSpec / 生成profile集合 | 合成人物の特性、ニーズ、provenance |
| 中核の世界（`sheltersim_core::World`） | 可変simulation state、将来event queue、実行済みevent履歴、モデル状態 |
| 保存したRun（形v6） | 目録（`journal.jsonl`）と部品（`data.pack`）。保存した点（`SavedPoint`）から復元、seek、Branch作成 |
| EffectProgram | 対象、時刻、仮定を持つ型付き・上限付き介入計画 |
| Project / Run / Branch | ローカルworkspace、Scenario版、Run状態、独立した履歴 |

主要schemaは個別に版管理しています。現在の形式にはScenarioSpec v3、ProfileSpec v2、保存したRunの形v6（`sheltersim-run-v6`）があります。PythonのCheckpointと形v1〜v5は読まず、計算し直してもらいます。契約変更時は現行validatorと移行テストを確認してください。版の異なるデータは互換とは限りません。

Scenario JSONとProject・Run・Branchの記録はJSONを使います。保存したRunの目録はJSONL、部品は中核のbinaryの形です（[project-persistence.md](project-persistence.md)）。Runの記録にはseed、モデル版、仮定、source ID、実行mode、hashを記録します。

## CLI

完全なcommand一覧は sheltersim --help で確認できます。

- prepare: Scenarioを検証し、previewを作成します。--freezeはpreview hashと結び付いた承認記録を作ります。
- run: 凍結済みScenarioを、Project（`--root`、既定`runs/.projects`。`--project-id`）のRun（`--run-id`）として、画面と同じ道で最後まで計算します（`POST {project}/runs`と`resume`。保存したRunの形v6、#450）。Projectがなければ作り、同じ内容のScenarioの版があればそれを使います。--offlineはProviderを呼ばないofflineの実験にします。出力はBranch `main`の記録、LLMの状態、心理モデルです。
- inspect: Runを画面と同じ答えで読みます。--record（Runの記録とBranch）、--evidence（モデル・出典・仮定）、--kpis（Branchの指標の時系列の1ページ。--start、--end、--step、--limit、--cursor）、--conversations（質問の記録）のどれか1つです。行動・判断の履歴、人ごとの履歴、原因のたどりは、#368の決定（履歴はkpisとconversationsだけ）でなくしました。
- compare: ProjectのBranch（`RUN`か`RUN/BRANCH`）を、指標の記録（`kind=kpis`の答え）で比べます。--baselineと--candidateはseedで組にし、同じ人のRunに限ります。組ごとに、指標が最初に違う分と、共通の最後の分の値と差を、組全体で各指標の差の要約（平均、標本標準偏差、近似95%区間）を出します。--metricで指標を絞り、--outputでJSONを書きます。
- review: 計算を終えたBranchのレビュー（画面の`GET …/branches/{branch}/report`と同じ）を出します。--comparisonに`compare`のJSONを渡すと、そのBranchを含む組を一緒に出します。
- project: Project、Run、Branch、履歴の永続操作を行います。
- model-catalog: 選択モデル、係数、単位、範囲、source分類を表示します。
- evaluate-models: 明示的に依頼されたoffline対応比較実験を、ProjectのRun（`<variant>-seed-<seed>`）として計算し、--outputに結果のJSONを書きます。
- web: ローカルのブラウザーの画面（`/`）とProject APIを起動します。Projectは`--runs-dir`の下の`.projects`に保存します。
- usage: CLI/Demoの利用記録と、PC全体の累積上限の記録を表示します。--unfreeze ACCOUNT_ID --confirm ACCOUNT_ID --reason TEXT で、凍結された利用記録を解除します（llm-integration.md参照）。

実行例はREADME.mdとdevelopment.mdにあります。

## HTTP route群

serverは許可されたfield/queryだけを受け付けます。実装はsrc/sheltersim/web/server.py、projects.py、project_services.py、workspace.pyにあります。#427から、画面が呼ぶ道の答えと誤りは中核の関数（`sheltersim_core::answers`、PyO3の`sheltersim_native.rules.answers`）が作り、ブラウザー版と同じbytesです。

| Route | 操作 |
|---|---|
| GET /api/v1/projects | ローカルProject一覧と保存容量を返します |
| /api/v1/projects/{project} | Project、Scenario、Run、証拠を参照します |
| /api/v1/projects/{project}/scenario-preview | Scenario案を作成・検証します |
| /api/v1/projects/{project}/scenario-approve | preview hashを承認し、保存ScenarioとRunを作成します |
| /api/v1/projects/{project}/runs/{run} | Runを再開・取消し・削除し、比較・証拠・利用記録を参照します。取消し（`POST …/cancel`）は `branch_id` を渡すとそのBranchの計算だけを、渡さなければRunのすべての計算を止めます。Runの参照の `branches` には、Branchごとの計算の状態（`status`）が付きます（#48） |
| /api/v1/projects/{project}/runs/{run}/branches/{branch} | timeline、履歴（`kind`は`kpis`と`conversations`だけ）、report、会話、LLM fallback集計（llm-status）を参照し、介入のpreview・apply、質問を送ります |
| /api/v1/usage/runtime | CLI（と以前の画面のDemo）の上限付き利用記録を参照します |
| GET /api/v1/llm | サーバーのAIの機能（LLM）がONかどうかを返します（`available`、`mode`、OFFの`reason`）。APIキーのあるrecord mode、またはreplay modeでONです。APIキーそのものは返しません。画面が上部とホームに出します（#581） |

Project APIは、中核が予定できるEffectProgramの操作の目録（`GET …/contracts`。#430から操作の一覧だけで、#584から中核の`program::catalog`の答え〔`schema_version` 3〕。ブラウザー版も同じものを返します）と、LLMの利用記録（`GET …/usage`、`GET …/runs/{run}/usage`）も扱います。Runの監査（`…/audit`）、履歴の索引（`…/index`）、Checkpointの一覧と節（`…/checkpoints`）、互換性（`…/compatibility`）、作り直し（`…/rerun`）、CLIの`project fork`・`project rerun`は、PythonのCheckpointと一緒に#427でなくしました。以前の画面だけが使った道（図面のupload・asset・Geometry、draft、evaluation、dataset、実験、複数Runの比較〔`comparison-query`〕、条件の差〔`condition-differences`〕、`first-divergence`、世帯の修正、標準施設の補正〔`facility-preview`〕、計測〔`…/telemetry`と、`resume`の`measure_performance`〕、介入なしの分岐〔`…/branches/{branch}/fork`〕、アーカイブ〔`POST …/archive`、`POST …/runs/{run}/archive`、`GET …/scenarios`、CLIの`project archive`。不要なRunは`POST …/runs/{run}/delete`で消します〕）は#376でなくしました。OpenAPI documentは別途生成していません。field・responseの正確な契約は、要求の型（Pythonの`InitialScenarioRequest`など）、中核の答え（`native/core/src/answers/`）とtestを参照してください。

### Projectと介入の流れ

- POST /api/v1/projects でローカルProjectを作成し、GETで一覧と容量を確認します。Project、Run、Branch IDは表示名ではなく不透明な識別子です。
- POST /api/v1/projects/{project}/scenario-preview はdraftを検証し、期限付きpreview IDと正準approval hashを返します。previewだけではRunを作らず、simulationも開始しません。
- POST /api/v1/projects/{project}/scenario-approve はpreview ID、一致するhash、approved: true、新しいRun IDを要求します。不変のScenario版と初期Run状態を保存します。
- Branchのintervention-previewは、指定時刻（`minute`）に、`program`か`text`のどちらか1つを受け取ります。`program`は画面が作用の目録（`GET …/contracts`）から組み立てたEffectProgramです（#584。形は[effect-program.md](effect-program.md#作用の目録584)）。中核の契約が断るprogramは400、`code: invalid_effect_program`で、理由を`error`に返します。`text`は文章で、APIキーがあればLLMで、なければ中核のofflineの読み取りでprogramにします（#463）。LLMの呼出しが失敗したときは`SHELTERSIM_LLM_ERROR_POLICY`に従います（`stop`なら409、`code: intervention_compiler_unavailable`）。ブラウザー版は中継の合言葉があるときだけAIで読み、中継が答えられないときはいつも`stop`と同じく理由を返して失敗します（#580）。どちらもその時刻の世界で検証し、元Branchは変えません。答えは`status`、`support_mode`、`mechanism_summary`、`limitations`、`questions`、`choices`、`fallback_used`、適用できるprogramなら`program`、`validation_issues`、`can_apply`、`preview_id`、`approved_program_hash`です（`text`の答えには`draft_plan`、`preview_notice`、`usage`も付きます）。intervention-applyはpreview ID、program hash、明示承認、child IDを要求し、元state hashを再検査して独立Branchを作ります。
- intervention-applyは、ブラウザー版と同じく、その場で子Branchを作ってから `{"status": "running", "branch_id": 子のID, "minute": 分岐時刻}` を返します（#427）。結果はRunの参照（`GET …/runs/{run}`）の `last_fork` に載ります。成功なら `{"status": "created", "branch_id", "minute", "state_hash"}`、失敗なら `{"status": "failed", "error", "code"}` です。`last_fork` は、そのサーバーのプロセスで最後の分岐の結果だけで、保存しません。
- 図面の取り込み（upload、plan-preview、Geometryの承認）と、scenario-previewの`plan_override`・`geometry_id`は#376でなくしました（決定3）。scenario-previewは標準の合成施設から作ります。
- timeline frameは指定tickの直前境界を表します。未計算・利用不可の履歴を推定値で返さず、要求を失敗させます。Branch比較は共通の計算済み時刻を使います。出力はsimulation上の記述で、外部データによる因果証拠ではありません。
- `GET /api/v1/projects/{project}/runs/{run}/replay-frames?branch_ids=A,B&start=M&end=N` は、1〜2個の異なるBranchについて、両端を含む最大8個の計算済み境界を連続読取りします。各queryは1回だけ指定します。`samples` はBranchが1個なら既存timeline frame、2個なら既存compare responseと同じ契約です。ただし各frameの`state_hash`は`null`です。再生では毎分のframeを作るので、そのたびに世界全体のdigestを求めると、frameを作る時間の大半を占めるためです（#500）。その分のdigestは、timelineかcompareで読めます。未計算範囲・fork前・重複Branch・読み取り中のhead変更は拒否します。記録済み応答だけで復元し、保存Branchを進めず、live Providerも呼びません。
- 今と違うモデルの版で計算したRunは閲覧のみです。timeline frame、replay-frames、比較の各frameには`model_view`（`view_only`、`recorded_version`、`installed_version`、`differences`、`message`）が付き、保存の間の時刻は直前の保存の時点を返して`saved_point_rounding`（`requested_minute`、`saved_minute`、`message`）を付けます。`minute`は表示した保存の時点です。resume、intervention-preview・applyは409、`code: run_model_view_only`で拒否します（[project-persistence.md](project-persistence.md#旧モデルのrun閲覧のみ)）。
- Branchへの質問は保存Run/Branchの証拠を読むだけで、simulation stateを変更しません。詳細はproject-persistence.md、adaptive-event-compiler.md、effect-program.mdを参照してください。

ブラウザーrouteは / だけです（画面のファイルは`demo-v2*`）。Project APIと承認契約を利用します（[対応表](ui-backup.md)）。`/index.html`は、クエリを保ったまま / へ302で送ります。前の道の`/demo-v2`と、以前の画面の道（`/demo`、`/admin`）は、旧版を使う人がいないので転送しません（#434）。以前の画面だけが使った `/api/v1/runs`（`sheltersim run --output` の成果物の参照）、`/api/v1/demo`、`/api/v1/nl`、`/api/v1/jobs`、`/api/v1/results-chat` はありません。CLIの`run`もProjectにRunを作るので、画面で開けます。ブラウザー版（静的サイト、#266）では、画面の中のWorkerが計算済みのRunの道（Project・Run・Branchの記録、timeline、1つの分岐のreplay-frames、evidenceなど）に同じJSONで答え、それ以外の道は「ブラウザー版では未対応」（409、`code`は`browser_unsupported`）を返します（[development.md](development.md#ブラウザー版のapi266)）。serverは既定で127.0.0.1にbindし、認証機能を持ちません。信頼できないnetworkへ公開しないでください。

POSTはsame-origin確認、JSON content type、request size上限、厳密なfield allowlistを使います。正確な上限とvalidatorはsource契約および回帰testで管理します。

## 状態変更とreplayの規則

Scenario作成とBranch状態変更には、確認済みhashに結び付く明示承認が必要です。serverは適用前にhash、元Branch、現在state、対象、上限、導入済みPolicyを再確認します。無効なplanは一部だけ適用されません。

過去timeline参照は記録済みstateとProvider応答を使います。不足履歴をlive modelで補いません。時刻Mはtick M直前の境界です。未計算時刻は厳密な状態として返しません。

## 正確な契約の参照先

- src/sheltersim/contracts: Scenarioの拡張の契約。
- src/sheltersim/experiments/scenario.py: ScenarioSpec。
- native/core/src: 中核の世界、EffectProgram（`program.rs`）と入力の契約（`contracts/`）、保存したRunの形（`run_store/`）。
- src/sheltersim/web/projects.py、project_services.py: Project HTTP API。
- tests/: request、validation、replay、互換性を確認する実行可能契約。
