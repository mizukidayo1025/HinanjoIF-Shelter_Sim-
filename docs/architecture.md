# アーキテクチャ

## コンポーネントの流れ

~~~text
CLI / ローカルHTTPサーバー（Python）          ブラウザー版（JS と WASM）
        |                                              |
Project、Scenario作成、地域データ、LLM Provider        |
        |                                              |
        +------------- PyO3 ----------+------- WASM ---+
                                      |
                 Rustの中核 sheltersim_core（native/core）
     0分の世界、tick（PlainRun）、EffectProgram、保存したRunの形v6、答え（answers）
                                      |
                 保存したRunの置き場 sheltersim_store（native/store、ファイル）／OPFS（ブラウザー）
~~~

Python標準ライブラリのHTTPサーバーがローカルブラウザーの画面（`/`）を配信します。ブラウザーは型付きrequestを送信し、simulation stateを直接書き換えません。読取りはstateを変えず、Project操作は別の検証済みcommandを使います。CLIの`run`・`inspect`・`compare`・`review`・`evaluate-models`も、同じProjectの道で計算し、読みます（`experiments/project_runs.py`、#450）。

計算（0分の世界、tick、介入、保存、答え）はRustの中核だけが行います（#368、#430）。PythonのWorldState、tickの規則、Checkpoint、履歴の索引は#430で消しました。Pythonに残るのは、HTTP、Scenarioの作成（`ScenarioSpec`、生成、プレビュー）、地域データ、LLMの接続（Provider、router、利用記録）です。

## モジュールの責務

| 領域 | 主な責務 |
|---|---|
| native/core | 世界の状態、tick、決定的乱数、EffectProgram、検証と契約、保存したRunの形、APIの答え（`native/README.md`） |
| native/store | 保存したRunのファイルの置き場（lock、容量、別プロセスの読み手、#426） |
| scenario_authoring | Scenarioを作る側の部品（#490）：`ScenarioSpec`が読む`model_configs`の節の型・検証・正準化（`configs/`。心理・社会モデルの係数、待ち行列の設定の確かめを含む）、Scenarioの作成が使う決定的乱数（`rng.py`）、Scenarioが予定するeventの種類（`event_kinds.py`）。モデルの定義と選択は中核のModel Catalog（`contracts/policies.py`が読む） |
| experiments | Scenario、生成、モデルの評価、Projectの道の計算（`project_runs.py`） |
| region | 地域データ、標準の合成施設とgeometry検証 |
| llm | Provider adapter、型付き出力、cache、応答再生、利用額記録、中核のLLMの口へのつなぎ（`llm/link.py`） |
| storage | Project、draft、利用記録の永続化（Runは`native/store`） |
| web | ローカルHTTP route、Project API、ブラウザーasset |

実装の正本はパッケージ構成です。公開入力の契約はsrc/sheltersim/contracts（Scenarioの拡張）とRustの中核（`native/core/src/contracts`）にあります。

## シミュレーションと状態境界

- 時刻は整数分です。event順と同順位の決着は、固定IDと明示的なsort keyで決めます。
- 中核の世界（`sheltersim_core::World`）が変更可能なsimulation stateの正本です。規則は型付きの提案を作り、中核が検証して適用します。
- 乱数はScenario seedと安定したdomain IDから決定します。simulation結果の判断にwall-clockやmodule-global乱数を使いません。
- inspection、描画、metricsは世界を変えず、simulation乱数も消費しません。
- Webサーバー（`serve_inspector`）の`ProjectAPI`は、計算1回（`POST R/resume`）ごとに別プロセスを起動して処理します（`web/worker_process.py`、`ProjectAPI(worker="process")`、#48）。起動はWindowsと同じspawn方式です。子プロセスがBranchの書込権（`ProjectFiles.claim_branch`、`branches/{branch}/worker-lock`）を自分で取り、取れたことをサーバーに知らせてから計算します。取れなければ`run_worker_busy`で断ります。計算は中核の`PlainRun`で60 tickずつ進め、ブラウザー版と同じ保存したRunの形（#427。今はv6）に書きます。中止は`multiprocessing.Event`で伝え、サーバーのプロセスがなくなった場合も、子プロセスは次のチャンクの境目で止まります。子プロセスが結果の状態を書かずに終わった場合（強制終了など）、サーバーはBranchを`interrupted`にします。同じRunのほかのBranchや異なるRunは、上限（`sheltersim web --max-parallel-runs`、既定2）まで同時に、それぞれ別プロセスで計算します。分岐（intervention-apply）はブラウザー版と同じく、サーバーのプロセスでその場で子Branchを作ります（分岐元のBranchが計算中なら断ります）。上限に達しているときの要求は、`background workers are busy` で断ります。テスト、CLI、計測器（`benchmarks/investigate_performance.py`）は、同じプロセスのスレッドで計算します（`worker="thread"`、既定）。

ScenarioSpec、中核の世界、EffectProgram、保存したRunの形、Project recordはそれぞれ個別に版管理します。古い形の保存（Pythonの Checkpoint と形v1〜v4）は読まず、計算し直してもらいます（#368の決定2）。

モデルの版は、保存したRunの`run`の行の`model_versions`に記録します（中核の`initial_model_versions`。#427より前はCheckpointの`model_signatures()`）。Policyと状態契約ごとの署名のほかに、モデル全体の版`engine:model`とRNGの`engine:rng_algorithm`（`CounterRNG.algorithm_id`）の2項目があります（中核の`native/core/src/model_version.rs`）。全体の版は、Scenarioごとの署名を変えずにすべてのScenarioの結果が変わる変更（libm、RNGのキー、規則の順序、すべてのRunが書く記録の中身）で1か所（`ENGINE_VERSION`）だけ上げます。今は版4です。版2では、exp、expm1、pow、fmaを固定した`libm`のcrateにし、情報のイベントの抽選のキーからイベントのIDを外しました。版3では、行動の記録（`agent_action_lifecycle`）を記録の版2にし、同じニーズの値を1回だけ書くようにしました（#255）。版4では、ローカル版・VM版の計算を中核の`PlainRun`と保存の形v5にし、tickの中のLLMの規則を中核に移しました（#427、#428。問い合わせの状態のhashが中核の世界のdigestになり、前のテープでは同じ問い合わせに答えられないため）。`engine:rng_algorithm`はseedとキーから値を引く方法で、Scenarioを作る抽選と共有するので、規則のキーを変えても上げません。2項目がない保存（2項目を記録する前の保存）は、版1と`sha256-keyed-v1`で計算されたものとして扱います。

記録した版が今のモデルと違うRunは**閲覧のみ**です。保存した時点はそのまま表示し、保存の間の時刻は直前の保存の時点に丸めて、丸めたことを応答に出します（`saved_point_rounding`。今のモデルで計算し直しません）。続きの計算、分岐、submit、EffectProgramの適用はエラーコード`run_model_view_only`と日本語の説明で止めます。閲覧はProviderを呼ばず、乱数も使いません。詳細は[project-persistence.md](project-persistence.md#旧モデルのrun閲覧のみ)です。

## 永続化とインターフェース

Project、承認済みScenario、Run、Branchとその保存した点・記録は、保存したRunの形v6でファイルに置きます（`native/store`、#426、#427）。CLIのRunも同じProjectに入ります。APIリファレンスを参照してください。

HTTP APIは永続Projectと状態変更操作（/api/v1/projects）と、利用記録の参照（/api/v1/usage/runtime）です。以前の画面のための保存Run成果物の参照（/api/v1/runs）とprocess内のデモBranch（/api/v1/demo）は#376でなくしました。

HTTPサーバーは既定で127.0.0.1にbindします。認証機能がないため、信頼できないネットワークへ公開してはいけません。

## 外部接続との境界

地域データProviderと任意Anthropic adapterはsimulation coreの外側にあります。Providerから得たデータは信頼せず、sourceと適用範囲を保持します。offline replayは保存済み応答か決定的fallbackを使い、過去時点の参照でlive Providerを呼びません。

モデル仮定と検証範囲はsimulation-models.mdとmodel-validation.mdを参照してください。
