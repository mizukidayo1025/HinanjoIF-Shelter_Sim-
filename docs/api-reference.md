# CLI・データ契約・HTTP API

## 版管理されるデータ

正準input/output schemaはパッケージ内で実装し、テストで確認しています。主なrecordは次のとおりです。

| Record | 用途 |
|---|---|
| ScenarioSpec | 固定された人口、避難所、schedule、モデル設定 |
| ProfileSpec / 生成profile集合 | 合成人物の特性、ニーズ、provenance |
| WorldState | 可変simulation state、将来event queue、実行済みevent履歴、モデル状態 |
| Checkpoint | 復元、seek、replay、Branch作成のための版管理snapshot |
| EffectProgram | 対象、時刻、仮定を持つ型付き・上限付き介入計画 |
| Project / Run / Branch | ローカルworkspace、Scenario版、Run状態、独立した履歴 |

主要schemaは個別に版管理しています。現在の形式にはScenarioSpec v3、ProfileSpec v2、拡張状態に応じてWorldState v5またはv6、Checkpoint v2（v1も読めます）があります。契約変更時は現行validatorと移行テストを確認してください。版の異なるデータは互換とは限りません。

Scenario JSON、Run manifest、CheckpointはJSONを使います。eventとdecisionはJSONL、metricsはCSVを使う場合があります。Run manifestにはseed、モデル版、仮定、source ID、実行mode、hash、LLM利用量とfallback集計（llm_usage.outcomes、llm-integration.md参照）を記録します。有効なsubsystemにより、追加の版管理済みtrace成果物が作られます。

## CLI

完全なcommand一覧は sheltersim --help で確認できます。

- prepare: Scenarioを検証し、previewを作成します。--freezeはpreview hashと結び付いた承認記録を作ります。
- run: 凍結済みScenarioを実行し、Run成果物を書きます。--offlineはProvider呼び出しを無効にします。
- inspect: profile、予定・実行済みevent、Agent履歴、証拠に結び付いた原因を検索します。
- compare: 保存済みRun directoryを比較します。
- project: Project、Run、Checkpoint、Branch、履歴の永続操作を行います。
- model-catalog: 選択モデル、係数、単位、範囲、source分類を表示します。
- evaluate-models: 明示的に依頼されたoffline対応比較実験を実行します。
- web: ローカルのブラウザーの画面（`/demo-v2`）とProject APIを起動します。Projectは`--runs-dir`の下の`.projects`に保存します。
- usage: CLI/Demoの利用記録と、PC全体の累積上限の記録を表示します。--unfreeze ACCOUNT_ID --confirm ACCOUNT_ID --reason TEXT で、凍結された利用記録を解除します（llm-integration.md参照）。

実行例はREADME.mdとdevelopment.mdにあります。

## HTTP route群

serverは型付きrequest契約と、許可されたfield/queryだけを受け付ける検証を使います。実装はsrc/sheltersim/web/server.py、projects.py、project_services.py、workspace.pyにあります。

| Route | 操作 |
|---|---|
| GET /api/v1/projects | ローカルProject一覧と保存容量を返します |
| /api/v1/projects/{project} | Project、Scenario、Run、asset、証拠を参照します |
| /api/v1/projects/{project}/scenario-preview | Scenario案を作成・検証します |
| /api/v1/projects/{project}/scenario-approve | preview hashを承認し、保存ScenarioとRunを作成します |
| /api/v1/projects/{project}/runs/{run} | Runを再開・取消し、比較・証拠・監査・利用記録を参照します。取消し（`POST …/cancel`）は `branch_id` を渡すとそのBranchの計算だけを、渡さなければRunのすべての計算を止めます。Runの参照の `branches` には、Branchごとの計算の状態（`status`）が付きます（#48） |
| /api/v1/projects/{project}/runs/{run}/branches/{branch} | timeline、履歴（`kind`は`kpis`と`conversations`だけ）、report、会話、LLM fallback集計（llm-status）を参照し、介入のpreview・apply、質問を送ります |
| /api/v1/usage/runtime | CLI（と以前の画面のDemo）の上限付き利用記録を参照します |

Project APIは登録モデル・契約（`GET …/contracts`）、Runの監査（`GET …/runs/{run}/audit`）、利用記録（`GET …/usage`、`GET …/runs/{run}/usage`）、履歴index、Checkpointも扱います。以前の画面だけが使った道（図面のupload・asset・Geometry、draft、evaluation、dataset、実験、複数Runの比較〔`comparison-query`〕、条件の差〔`condition-differences`〕、`first-divergence`、世帯の修正、標準施設の補正〔`facility-preview`〕、計測〔`…/telemetry`〕、介入なしの分岐〔`…/branches/{branch}/fork`〕）は#376でなくしました。OpenAPI documentは別途生成していません。field・responseの正確な契約は現在のPydantic modelとtestを参照してください。

### Projectと介入の流れ

- POST /api/v1/projects でローカルProjectを作成し、GETで一覧と容量を確認します。Project、Run、Branch IDは表示名ではなく不透明な識別子です。
- POST /api/v1/projects/{project}/scenario-preview はdraftを検証し、期限付きpreview IDと正準approval hashを返します。previewだけではRunを作らず、simulationも開始しません。
- POST /api/v1/projects/{project}/scenario-approve はpreview ID、一致するhash、approved: true、新しいRun IDを要求します。不変のScenario版と初期Run状態を保存します。
- Branchのintervention-previewは、指定時刻に自然言語または型付きprogramを受け取り、分離したstateで検証します。元Branchは変えません。自然言語の変換がruleの結果になった場合（`fallback_used: true`）、Opusを使わなかった理由を`limitations`に含めます。`SHELTERSIM_LLM_ERROR_POLICY=stop`では、ruleの結果の代わりに409（`code: intervention_compiler_unavailable`）で理由を返します（llm-integration.md参照）。intervention-applyはpreview ID、program hash、明示承認、child IDを要求し、元state hashを再検査して独立Branchを作ります。
- intervention-applyは、Webサーバーでは別プロセスで子Branchを作り、すぐに `{"status": "running", "branch_id": 子のID, "minute": 分岐時刻}` を返します（#48）。時刻の範囲、子IDの重複、program hashの不一致は、その場で拒否します。作っている間、Runの状態は `running` です。終わると元の状態に戻り、Runの参照（`GET …/runs/{run}`）の `last_fork` に結果を載せます。成功なら `{"status": "created", "branch_id", "minute", "state_hash"}`、失敗なら `{"status": "failed", "error", "code"}` です。`last_fork` は、そのサーバーのプロセスで最後に始めた分岐の結果だけで、保存しません。CLIとテスト（`ProjectAPI(worker="thread")`）では、従来どおり子Branchを作り終えてから `{"branch_id", "minute", "state_hash"}` を返します。CLIの`project fork`は、介入なしの子Branchを`TimelineStore.fork`で作ります。
- 図面の取り込み（upload、plan-preview、Geometryの承認）と、scenario-previewの`plan_override`・`geometry_id`は#376でなくしました（決定3）。scenario-previewは標準の合成施設から作ります。
- timeline frameは指定tickの直前境界を表します。未計算・利用不可の履歴を推定値で返さず、要求を失敗させます。Branch比較は共通の計算済み時刻を使います。出力はsimulation上の記述で、外部データによる因果証拠ではありません。
- `GET /api/v1/projects/{project}/runs/{run}/replay-frames?branch_ids=A,B&start=M&end=N` は、1〜2個の異なるBranchについて、両端を含む最大8個の計算済み境界を連続読取りします。各queryは1回だけ指定します。`samples` はBranchが1個なら既存timeline frame、2個なら既存compare responseと同じ契約です。未計算範囲・fork前・重複Branch・読み取り中のhead変更は拒否します。記録済み応答だけで復元し、保存Branchを進めず、live Providerも呼びません。
- 今と違うモデルの版で計算したRunは閲覧のみです。timeline frame、replay-frames、比較の各frameには`model_view`（`view_only`、`recorded_version`、`installed_version`、`differences`、`message`）が付き、保存の間の時刻は直前の保存の時点を返して`saved_point_rounding`（`requested_minute`、`saved_minute`、`message`）を付けます。`minute`は表示した保存の時点です。resume、fork、intervention-preview・applyは409、`code: run_model_view_only`で拒否します（[project-persistence.md](project-persistence.md#旧モデルのrun閲覧のみ)）。
- Branchへの質問は保存Run/Branchの証拠を読むだけで、simulation stateを変更しません。詳細はproject-persistence.md、adaptive-event-compiler.md、effect-program.mdを参照してください。

ブラウザーrouteは /demo-v2 だけです。Project APIと承認契約を利用します（[対応表](ui-backup.md)）。`/` と、以前の画面の道（`/demo`、`/admin`、`/index.html`）は、クエリを保ったまま /demo-v2 へ302で送ります（#376）。以前の画面だけが使った `/api/v1/runs`（`sheltersim run --output` の成果物の参照）、`/api/v1/demo`、`/api/v1/nl`、`/api/v1/jobs`、`/api/v1/results-chat` はありません。CLIのRunは `sheltersim inspect` で確かめます。ブラウザー版（静的サイト、#266）では、画面の中のWorkerが計算済みのRunの道（Project・Run・Branchの記録、timeline、1つの分岐のreplay-frames、evidenceなど）に同じJSONで答え、それ以外の道は「ブラウザー版では未対応」（409、`code`は`browser_unsupported`）を返します（[development.md](development.md#ブラウザー版のapi266)）。serverは既定で127.0.0.1にbindし、認証機能を持ちません。信頼できないnetworkへ公開しないでください。

POSTはsame-origin確認、JSON content type、request size上限、厳密なfield allowlistを使います。正確な上限とvalidatorはsource契約および回帰testで管理します。

## 状態変更とreplayの規則

Scenario作成とBranch状態変更には、確認済みhashに結び付く明示承認が必要です。serverは適用前にhash、元Branch、現在state、対象、上限、導入済みPolicyを再確認します。無効なplanは一部だけ適用されません。

過去timeline参照は記録済みstateとProvider応答を使います。不足履歴をlive modelで補いません。時刻Mはtick M直前の境界です。未計算時刻は厳密な状態として返しません。

## 正確な契約の参照先

- src/sheltersim/contracts: APIとstate model。
- src/sheltersim/experiments/scenario.py: ScenarioSpec。
- src/sheltersim/core/world.py、checkpoint.py: 永続simulation state。
- src/sheltersim/events/effects.py: EffectProgram schemaとinterpreter。
- src/sheltersim/web/projects.py、project_services.py: Project HTTP API。
- tests/: request、validation、replay、互換性を確認する実行可能契約。
