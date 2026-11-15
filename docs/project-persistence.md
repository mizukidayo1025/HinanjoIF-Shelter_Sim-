# Projectの永続化、正確な時刻、復旧

Web server（`sheltersim web`、ローカル版・VM版）はRUNS_DIR/.projectsを開き、ProjectAPI（`src/sheltersim/web/projects.py`）経由で/api/v1/projectsを提供します。#427から、記録と計算の結果は**ブラウザー版と同じ置き場と形**（形v6、`sheltersim-run-v6`）です。Projectのフォルダーの中身（`project.json`、Scenarioの版、`run.json`、`evidence.json`、Branchごとの`branch.json`・`journal.jsonl`・`data.pack`）は下の「ブラウザーのProject記録と1つのファイル」の表のとおりで、Rustの置き場（`native/store`、下の「ディスクの置き場」）が書き、読みます。答えは中核の関数（`sheltersim_core::answers`、#449）が作り、ブラウザーと1 byteも違いません。Pythonに残るのは、要求の振り分け、答えに要るファイルを読むこと、計算の順番、どのRunをどの分に持っておくか（ブラウザーのJSと同じ役目）と、Pythonだけが作るもの（文章からのScenario、`scenario-preview`）です。

- **Runを作る**（`POST {project}/runs`、`scenario-approve`）：承認したScenarioを新しい版として保存し、Runの記録（`model_versions`と`evidence.json`はPythonが作ります）と、Branch `main` の0分（Runが作る0分の世界の書き出し、`export`の行）を保存します。Scenario版とRun入力は不変です。
- **計算**（`POST R/resume`、`cancel`）：Branchの先頭（記録の`checkpoint_hash`が指す目録の行）から、中核の`PlainRun`で60 tickずつ計算し（`web/worker_process.py`）、360分ごと、目標の分、時計の終わりに保存する点を書いて、Branchの記録をそこへ進めます。記録が計算の確定点で、その後ろの行は次の計算の始めに切り落とします。Webサーバーでは計算1回ごとに別のプロセス（spawn）で、上限（`--max-parallel-runs`、既定2）まで同時に計算します。CLIとテスト（`ProjectAPI(worker="thread")`）は同じプロセスのスレッドで1つずつ計算します。中止はチャンクの間で止まり、その分に保存する点を書きます。チャンクのtickは置き場のtransaction（`.lock`）の外で進め、そのチャンクのファイルを書くとき（保存する点、Branchの記録の移動）だけtransactionを持ちます（`Computation.advance`と`store`）。APIキーのあるRunでは、tickの中のLLMの呼び出しが利用の記録（`UsageLedger.reserve`）のために同じtransactionを取るためです。transactionを持ったままtickを進めると、同じプロセスの中で自分の持つlockを待ち続けていました（#465）。分岐を書くのはwriterのlease（`worker-lock`）を持つ1つの計算だけなので、tickの間にほかの書き手が入ることはありません。
- **Runの状態**：Branchごとの状態は`branch.json`の`status`、Runの状態は`run.json`の`status`です。どれかのBranchが計算中ならRunも`running`、そうでなければ最後に止まったBranchの状態です。サーバーの起動時の回復は、計算中のまま書込権を持つプロセスがいないBranchとそのRunを`interrupted`にします。プロセスが状態を書かずに終わった（強制終了など）ときも、サーバーがBranchを`interrupted`にします。
- **分岐**（`intervention-apply`）：ブラウザーと同じく、その場で子Branchを作ってから答えます。親をその分に戻して（保存した点から計算し直して）、プレビューのときの世界のhashと比べ、EffectProgramを当て、子の保存したRunを書きます。子は親の部品を写さずに参照します（#368の決定6）。結果はRunの参照（`GET …/runs/{run}`）の`last_fork`に、そのサーバーのプロセスが持ちます。
- **読む**（timeline、replay-frames、比較、指標の時系列、レポート、質問）：その分のRunを、保存した点に戻して計算し直して作ります（Runごとに2つまで持ち、後の分は持っているRunを進めます。ブラウザーの`BrowserApi.simulationAt`と同じ）。過去の読み出しはProviderを呼ばず、状態も乱数も変えません。
- **質問・PDF・レポート**：答えはブラウザーと同じです。質問はLLMなしの決まった答えと、質問の記録（保存したRunの`conversations`の部品）です。PDFは「Opusの記録応答またはAPI接続が必要です」（409 `pdf_analysis_offline`）で断ります。レポートは、初めて求められたときに、Runの終わりの世界から規則で作って`report.json`に残します。#427より前のサーバーにあった、APIキーがあるときの質問・PDF・レポートの分析と、介入の文章のLLMでの読み取りは、中核にLLMの口ができるまで（#368）ありません。
- **LLMの記録**：APIキーがあるサーバーで作ったRunは、そのLLMの設定（mode、offlineの実験か）を`runs/{run}/llm.json`に持ちます。利用の記録（`usage/`、`runs/{run}/usage.json`）と合わせて、Pythonのサーバーだけのファイルで、1つのファイル（`.shelterproj`）には入りません。
- **古い形のRun**：#427より前にPythonが保存したRun（Checkpointと`history.sqlite`）は読みません。開くと409 `stored_run_outdated`（「古い保存の形（Python の Checkpoint）…計算し直してください」）になります。offlineのRunは、Scenarioから作り直せます（#368の決定2）。

同じ操作をCLIから利用できます（画面と同じ道を通ります）。

~~~text
sheltersim project --root runs/.projects create --project-id study
sheltersim project --root runs/.projects save-scenario --project-id study --scenario scenario.json --approved-hash <reviewed-canonical-sha256>
sheltersim project --root runs/.projects new-run --project-id study --run-id trial --scenario-id <id> --scenario-version 1
sheltersim project --root runs/.projects resume --project-id study --run-id trial --minute 100
sheltersim project --root runs/.projects frame --project-id study --run-id trial --minute 73
~~~

approved hashは確認した正準ScenarioSpecそのものに結び付きます。時刻Mはtick Mの直前境界です。frameには`state_hash`（その分の世界のdigest）と`computed_until`を含み、未計算時刻は拒否します。親と兄弟のstateは独立しています。容量の上限（`--project-storage-gb`）は、書く前に確かめます。100人・7日のRunは、保存したRunで約7 MB（目録とpack）です（#427の計測。#427より前は552 MB）。

## 旧モデルのRun（閲覧のみ）

保存したRunは、計算に使ったモデルの版を目録の`run`の行の`model_versions`に記録します（モデル全体の版`engine:model`とRNGの`engine:rng_algorithm`を含みます。仕組みは[architecture.md](architecture.md)）。中核が今そのScenarioについて記録する版と違うRunは閲覧のみです（中核の`answers::frames::model_view`、`model_version`。ローカル版・VM版もブラウザー版も同じ関数です）。

- **読める：** frame（`GET …/timeline?minute=M`）、replay-frames、比較、指標の時系列、レポート、会話の記録。
- **保存の間の時刻：** 今のモデルで計算し直さず、その分以前で一番近い保存した点を表示します（D2）。frameには、`model_view`（記録した版、今の版、違う項目、日本語の説明）と、丸めたときは`saved_point_rounding`（`requested_minute`、`saved_minute`、日本語の説明）が入ります。
- **止める：** 続きの計算（`POST R/resume`、CLIの`resume`）、介入の確認と適用（intervention-preview、intervention-apply）。HTTPは409、`code`は`run_model_view_only`です。計算を始める前に止め、保存したファイルは変えません。

## ブラウザーのProject記録と1つのファイル（#265）

ブラウザー版（#132の段階6）は、Project・Run・Branchの記録をOPFSのフォルダーに置き、画面が呼ぶ`GET /api/v1/projects`、`POST /api/v1/projects`、`GET {project}`、`GET {project}/runs`、`GET {project}/runs/{run}`、`GET {project}/runs/{run}/evidence`に、Pythonのserverと同じ形のJSONで答えます（`native/wasm/js/project_store.mjs`が記録のファイルを読み、中核の`sheltersim_core::answers::records`が答えを書きます。#449）。#427から、ローカル版・VM版も同じフォルダーと記録を使います（Rustの置き場、下の「ディスクの置き場」）。

| 置き場所（Projectのフォルダーから） | 中身 |
|---|---|
| `project.json` | `schema_version`、`project_id`、`archived`（アーカイブは#376でなくしたので、いつも`false`） |
| `scenarios/<scenario>/v<n>.json` | Scenarioの記録（`schema_version`、`scenario_id`、`scenario_version`、`scenario_hash`、`approved`）。Scenario自体は持たない |
| `scenarios/<scenario>/v<n>.scenario.json` | Pythonが書いたScenarioの正準JSONのbytes。sha256が`scenario_hash` |
| `runs/<run>/run.json` | Runの記録（Pythonの`RunReference`の欄と`status`、`archived`、`created_at`）。`schema_version`は2で、`created_at`はRunを作った時刻（UTC、秒まで、`YYYY-MM-DDTHH:MM:SSZ`、#447）。時刻は作る側が渡します（ブラウザーは`ScenarioRoutes`の`now`、ローカル版・VM版は`ProjectAPI`の`clock`）。#447より前の記録は`schema_version`1で`created_at`を持たず、そのまま読みます |
| `runs/<run>/evidence.json` | Pythonのserverが`runs/{run}/evidence`に返すJSONのbytes（`evidence_context`と、その道が足す一覧） |
| `runs/<run>/branches/<branch>/branch.json` | Branchの記録（Pythonの`BranchReference`の欄と`status`） |
| `runs/<run>/branches/<branch>/journal.jsonl`、`data.pack` | Branchの保存したRun（#252。形は中核の`sheltersim_core::run_store`、読み書きは`run_store.mjs`） |

記録は、鍵を並べたJSONと改行です。ScenarioとevidenceはPythonが書いたbytesのまま持ち、答えるときもそのまま埋め込みます（JSで読み直すと`1.0`が`1`になるため）。案A（#244）のあいだは、世界を作るのも、Scenarioとevidenceを作るのもPythonです。

Pythonの欄のうち、ブラウザーで意味が変わるもの：

| 欄 | ブラウザーで入るもの |
|---|---|
| Branchの`checkpoint_hash` | 先頭（`computed_until`の世界）を記録した目録の行の「先頭のhash」：行のJSONから圧縮後の場所と長さ（`offset`・`length`）を除いたもののsha256（#368）。圧縮の仕方が違っても同じ値です。保存したRunの最後の`end`の行で、分とdigestを持つ。ブラウザーで途中まで計算したBranch（#267）では、その分の途中の点の`saved`の行（0分なら`export`の行）。計算前は`null`。画面は先頭が変わったかどうかにだけ使う |
| Runの`status` | `run.json`に持つ（Pythonは`status.json`に分ける） |
| Projectの一覧の`storage` | `bytes`と`files`は置き場のファイルの合計、`limit_bytes`は呼ぶ側が渡す値（ページでは`navigator.storage.estimate()`の`quota`） |
| Runの`last_fork` | 返さない（分岐は#267） |
| Branchの`status` | いつも持つ（Pythonは計算したことのあるBranchだけ）。ブラウザーで作った計算前のRun（#267）は、RunもBranchも`queued` |

誤りは`ProjectStoreError`で、`message`はPythonの`StorageError`と同じ文（あれば）、`code`はその符号です。

1つのファイル（`sheltersim-project-bundle`、版2、拡張子`.shelterproj`）は、Projectのフォルダーのファイルをそのまま並べたものです。中のBranchの保存したRunを形v5（#368）にしたので、版を2に上げました。版1のファイルは古い形のRunを持つので読まず、「版1は読めない」という誤りにします（計算し直します）。形v6（#478）では版を上げていません。版2のファイルの中のRunが形v5なら、読み込むときにRunの`stored_run_outdated`の誤りになります。先頭から、16 bytesの印（`0x89`、`SHELTERSIM-PRJ`、`0x0a`）、版（4 bytes、big-endian）、目録の長さ（4 bytes）、目録のsha256（32 bytes）、目録（JSON：`format`、`version`、`project_id`、作った道具の`source`、各entryの`path`・`length`・`sha256`）、各entryのbytesの順です。Branchの保存したRunは、目録のpieceの種類を読まずにファイルごと運ぶので、#251が足す途中の世界もそのまま入ります。

読み込み（`importProject`）は、印、版、目録とentryのsha256、記録どうしの関係（Scenarioのhash、RunのScenario、Branchの親）を確かめます。続けて、各Branchの保存したRunを、メモリーの中に置いたentryから読み戻し、digest（`readAndVerify`。分岐の子は、親のentryの部分も読みます）と記録の先頭を確かめます。すべて合ってから書きます。どれかが合わなければ誤りにし、何も書きません。`project.json`は最後に書くので、途中で止まったProjectは一覧に出ません。同じIDのProjectがあれば断ります。`replace`では、ファイルを確かめ終えてから古いものを消して書きます。そのため、確かめで断られたときは古いProjectがそのまま残ります（#273）。消した後に書き込みそのものが失敗したとき（容量が足りないなど）は、古いProjectは戻らず、新しいものも残りません。違う版のファイルは「版Nは読めない」という誤りにします。

| 関数 | 使う所 |
|---|---|
| `memoryTree()`、`opfsTree(directory)` | 置き場 |
| `createProject`、`saveScenario`、`createRun`、`setRunStatus`、`writeBranch`、`computeBranch`、`branchBackend` | 記録を作る、Branchを計算して残す |
| `projectsResponse`、`projectResponse`、`runsResponse`、`runResponse`、`evidenceResponse` | 道の答え（JSONの文字列） |
| `exportProject`、`importProject`、`readBundle`、`packBundle` | 1つのファイル |

計算済みの例は`scripts/build_project_sample.py --bundle`で作ります（[development.md](development.md#ブラウザー向けのwasm試作)）。

### ブラウザーで計算するBranchの先頭（#267）

ブラウザーで計算する（`POST runs/{run}/resume`、`native/wasm/js/compute.mjs`）Branchの記録は、保存したRunの目録の「先頭にできる行」を指します。先頭にできるのは、`export`の行（0分）、`saved`の行（どれも、その前にログと標本の塊がすべて保存されています）、`end`の行です。目録の行を書いて書き込みを終えて（flush）から記録を書くので、記録が計算の確定点です。記録の先頭より後の行は、次の先頭まで届かなかった計算で、続けるときに切り落とします（目録を先に、続けて`data.pack`の、どの行も指さないbytesを切ります）。先頭にできる行と、切り落とす長さは中核が決めます（`run_store::Journal::heads`、`Journal::cut`、#368）。そのBranchから分岐した子が、先頭より後の行まで親の部分を読む場合は、切り落とさずに断ります（子は親の部分を写さずに参照するので、#368の決定6）。

途中の点を先頭にできるよう、点を保存する前に、いつもログと標本の塊を閉じます（中核の`RunWriter::save`、`Blocks::seal`。形v5からは`runAndStore`も同じで、目録の`saved`の行の塊の数は、その行の前に保存した塊の数と等しくなります。#368の決定2）。閉じた後の記録は新しい塊に入るので、点の前に保存した塊だけで点に戻せ、戻したRunはそのまま後ろへ塊を足していけます（#251の`Restore`は、戻したRunの塊をすべて閉じたものとして持ちます）。塊の区切りが変わるだけで、記録、標本、digestは変わりません。続けるときは、先頭の点に戻したRunの表と塊の数が、目録のその点までの表と塊の行と合うことを確かめ（`RunWriter::after`と`fits`）、合わなければ続けずに`failed`にします。

ブラウザーでの`resume`・`cancel`の答えは、Pythonのserverと同じstatus、JSON、誤りの文と符号です。違う所：`target_minute`の`60.0`のようなJSONの小数は、JSでは整数と区別できないので受け付けます（Pythonは断ります）。別のモデル版で計算したRunは、Pythonと同じく閲覧のみです（#368の決定8）。保存したRunの`run`の行の`model_versions`を、中核が今そのScenarioについて記録する版（`modelVersions`）と比べ（規則と文はPythonの`model_version.py`と同じ中核の`model_version`）、違えば`resume`、`intervention-preview`、`intervention-apply`を409の`run_model_view_only`で断ります。閲覧はできます。保存の間の分は、Pythonと同じく今のモデルで計算し直さず（D2）、その分以前で一番近い途中の点（0分からのRunでは最初の点の前は0分）の世界を表示し、frameに`model_view`（記録した版と今の版、違う項目、日本語の説明）と、丸めたときは`saved_point_rounding`（`requested_minute`、`saved_minute`、「旧モデルのRunのため、…分は計算し直さず、保存した時点（…分）の状態を表示しています。」）を足します。timeline、replay-frames、比較が同じframeを使います（どの点を見せるかと足す欄は中核の`Journal::saved_minute_at`と`ModelStatus::describe_json`が決めます）。違う所：中核が署名できないScenario（中核の表にないもの。試験の世界だけ）では、Pythonのように閲覧のみにせず、エンジンの版（`engine:model`・`engine:rng_algorithm`）だけを比べます。`measure_performance`は真偽値か確かめるだけで、計測は記録しません。`400`の誤り（JSONでない本文）は`"code": null`を持ちます（#266の`POST /api/v1/projects`と同じ。Pythonは`code`を持ちません）。

### ディスクの置き場（Rust、#426）

ローカル版・VM版がブラウザーと同じ置き場に計算を書き、同じ置き場から読むための部品が、`native/store`（crate `sheltersim_store`）です。中核（`sheltersim_core`）はファイルを読み書きせず、WASMの build にも入らないので、ファイル・圧縮・lock・容量の処理はこのcrateに置きます。Pythonからは`sheltersim_native.rules.run_files`（`ProjectFiles`、`Lease`、`StoredRun`、`Computation`）で呼びます。Pythonのserverの計算と読み出しは、#427からこれを使います（上の「Projectの永続化」）。

- **フォルダーと記録**：上の表と同じです（`records.rs`）。記録は鍵を並べたJSONと改行で、ブラウザー・Pythonと同じbytesです。ファイルは一時ファイルに書いて flush してから名前を替え、フォルダーも flush します（Pythonの`ProjectStore._write`と同じ）。誤りの文はPythonの`StorageError`と同じです。
- **計算の段取り**：Branchの先頭まで戻して（切り落とし、戻し、判断の記録を集める）区切って進めること、保存する点・目標・終わりで記録の先頭を動かすこと、止めること、中断したBranchを`interrupted`にすること、分岐を作ること、質問の会話を残すこと、0分から最後まで計算することは、中核の`sheltersim_core::compute`（#476）がI/Oをしない「段取りの機械」（`Procedure`）として決めます。機械は「この記録を読む」「この部品を読む」「この部品を書く」「この行を足す」「この記録を書く」「計算する」といった指示（`Step`）を返し、ホストが行った結果を受け取って次へ進みます。`compute.rs`はその指示をファイル・flush・lock・容量で行うだけです。tickは`Computation::advance`だけで進み、その後の指示（`store`）だけを置き場のtransactionの中で行います（#465）。
- **保存したRun**：部品をdeflate-raw（flate2）で圧縮して`data.pack`の末尾に書き、ディスクまで flush してから、目録の行を`journal.jsonl`に書いて flush します（`compute.rs`、`stored.rs`）。圧縮後のbytesはブラウザーと違うことがありますが、先頭のhashと圧縮前の中身は同じです。
- **別のプロセスの読み手**：目録を1度読み、最後の欠けていない行までを読みます。書き手が追記している途中の行や、強制終了で欠けた行は読みません。読んだ行が指すbytesは、もうディスクにあります。続きの計算は、Branchの記録の先頭より後の行（欠けた行を含む）を切り落としてから始めます。
- **lock**：Pythonの`claim_branch`・`claim_run`・`transaction`と同じファイル（`.lock`、Runと各Branchの`worker-lock`）に、同じ種類のlockをかけます（Linux・macOSは`flock`、Windowsは`msvcrt.locking`が取る先頭のbyteに重なる`LockFileEx`）。PythonとRustのどちらが持っていても、もう一方は断られます。同じBranchの計算は1つです。
- **容量の上限**：`--project-storage-gb`の上限を、書く前に確かめます（`quota.rs`）。書き手は始めに置き場を測り、その後は自分の書いた量を足していきます。上限で止まった計算は、書き終えた行までが残り、記録は最後の先頭のままです。
- **答えが読むファイル**：中核の答え（#449）が読む記録を、ブラウザーの`project_store.mjs`と同じ集め方で読みます（`listing.rs`）。IDでない名前のファイルは無いものとし、目録は最後の欠けていない行までを読みます。介入のプレビューの下書き（`drafts/<id>.json`）と、初めて求められたときに作るレポート（`report.json`）もここで書きます。
- **Runの状態**：Branchの状態を書くと、Runの状態をそのまとめにします（Pythonの`set_branch_status`と同じ。どれかのBranchが計算中ならRunも計算中）。計算の終わり（`Computation::end`）も同じです。
- **古い形のRun**：#427より前にPythonのserverが保存したRun（Runのフォルダーに`checkpoints/`があり、Branchに目録がないもの）は、ブラウザーの形v1〜v4と同じく`stored_run_outdated`の誤り（「古い保存の形（Python の Checkpoint）…計算し直してください」）にします。
- **LLMの記録**：APIキーのあるサーバーの計算は、中核のLLMの口（`Computation.set_llm`、PythonのLLMRouterを`RouterLink`で、#428）を使い、問い合わせを保存したRunのLLMのテープ（`llm_tape`の部品）に残します。テープのあるRunを途中の分に戻すときは、戻した点より後の問い合わせにテープから答えます（Providerは呼びません）。
- **1つのファイル**：Projectを1つのファイル（`.shelterproj`、ブラウザーの`exportProject`と同じ形の版2）に書きます（`bundle.rs`）。Pythonのserverだけが持つファイル（lock、下書き、LLMの利用の記録・テープ・設定）は入れません。それ以外の知らないファイルは、ブラウザーと同じく断ります。
- **確かめ**：`cargo test -p sheltersim_core --test core compute::`は、メモリーのホストで段取りの機械を動かし、0分から計算したRunと区切って計算したRunがブラウザーの保存したRunとbyteまで同じになること、記録の先頭の動き、欠けた行の切り落とし、途中で止めたときの保存、分岐・質問・中断の手順、断る文を確かめます。`cargo test -p sheltersim_store`は、`native/wasm/tests/node_stored_runs.mjs`の5つの保存したRunを、この置き場で作り直します。fixtureと同じ圧縮（deflateのstored block）では、目録・`data.pack`・記録（`stored_runs/records.json`）がブラウザーとbyteまで同じです。flate2では、各行の先頭のhashと圧縮前の部品が同じです。`tests/integration/test_run_files.py`は、PythonとRustのlockが互いに断ること、追記中に別のプロセスが読めること、Branchを持ったまま強制終了した書き手のlockが外れ、欠けた行が読まれず、続きの計算で最後まで同じ結果になることを確かめます。

## ブラウザーの時系列・レポート・LLMの状態（#266）

ブラウザー版は、Branchの`GET …/history?kind=kpis`、`GET …/report`、`GET …/llm-status`に、同じRunにPythonのserverが返すのと同じbytesのJSONで答えます（中核の`sheltersim_core::answers`。ブラウザーは`native/wasm/js/api_answers.mjs`がファイルを読んで呼びます。#449）。

| 関数 | 答える道 |
|---|---|
| `historyResponse({wasm, tree, projectId, runId, branchId, query, cache})` | `history`。`query`は`?`の後ろの文字列（Pythonの`parse_qs`と同じ読み方。空の値は無い扱い）。`cache`は`historyCache()`で、読み戻した標本をBranchの先頭ごとに持ちます |
| `reportResponse({wasm, tree, projectId, runId, branchId, query})` | `report` |
| `llmStatusResponse({tree, projectId, runId, branchId, query})` | `llm-status` |
| `errorAnswer(error)` | 誤りを`{status, body}`にします。`ApiError`はその`status`、`ProjectStoreError`はserverの`StorageError`と同じ409と`code`、そのほかは500 |

どれも答えのJSONの文字列を返し、断るときは`ApiError`（`status`、`message`がserverの`error`、`code`）を投げます。serverと同じく、問い合わせの欄の誤り（知らない欄、同じ欄が2つ）は409の`invalid_query_fields`、契約に合わない値は400の`invalid_input`、計算した範囲の外は409の`history_outside_computed_range`、合わないcursorは409の`stale_or_foreign_cursor`です。

時系列は、Pythonの履歴の索引ではなく、Branchの保存した指標の標本（#252）から作ります。行は索引と同じで（`MetricSnapshot`の欄から`source_ids_json`を除き、資源の台帳の列、`time_semantics`、`record_kind`、`run_id`、`branch_id`を足したもの）、`start`から`end`までの`start + k*step`の分を、cursorの後から`limit`件返します。資源の台帳の列は、標本の形`sheltersim-browser-run-v3`から標本が持ちます（[development.md](development.md#ブラウザー向けのwasm試作)）。

Pythonの欄のうち、ブラウザーで意味が変わるもの：

| 欄・動き | ブラウザーで入るもの |
|---|---|
| 時系列の`anchor_hash`とcursorの`anchor` | Branchの先頭（記録の`checkpoint_hash`）。cursorのほかの中身（`scope`、分、key）はPythonと同じ |
| 時系列の`indexed_through_minute` | Branchの`computed_until`（保存したRunは先頭まで標本を持つので、索引の遅れはありません） |
| 時系列の`first_indexed_checkpoint_minute` | Branchの`fork_minute`（保存したRunの始まり） |
| `kind`が`kpis`以外の時系列 | 409、`browser_unsupported`（範囲などのserverの確かめの後） |
| 古い形（v1〜v4）で保存したBranch | どの道でも409、`stored_run_outdated`（「このシナリオの計算結果は古い保存の形（…）で保存されています。…計算し直してください。」。#368の決定7） |
| 保存したRunのないBranch | `anchor_hash`と`indexed_through_minute`と`first_indexed_checkpoint_minute`が`null`、行なし |
| レポート | serverは計算が終わったときに書きます。ブラウザーは、終わったBranchで最初に聞かれたときに、保存したRunの最後の分から作り（最後の点に戻すので、100人・7日のNodeで約4秒）、Branchのフォルダーに`report.json`として残します。終わっていなければ`{"status":"not_generated"}` |
| LLMの状態の`checkpoint_hash` | Branchの先頭。`mode`は`offline`、記録はありません |

レポートは、画面のofflineのRun（`scenario-approve`から作り、`SHELTERSIM_LLM_MODE=offline`で計算したもの）でserverが書くものです。評価の依頼はofflineで規則のfallbackになり（`rule_fallback`）、最後の指標の標本、最後の100件の記録のうち重要なもの20件、Scenarioから決まる仮定のID、世界のdigest（`state_hash`）を持ちます。`execution_mode: offline`で作ったRun（実験用、`offline_experiment_no_paid_review`）の答えではありません。

履歴のcursorは、問い合わせ（Project、Run、Branchと、`cursor`・`limit`を除く`HistoryQuery`の欄）のdigest（`scope`）と先頭に結び付いています。同じBranchでも別の問い合わせのcursorは`stale_or_foreign_cursor`で断り、`limit`だけが違う問い合わせには使えます（#278）。ブラウザーも同じdigestを作ります。

`native/wasm/tests/node_api_answers.mjs`が、`native/core/tests/data/api_answers.json`のRunで、serverの答えと同じbytesになることを確かめます。

## ブラウザーの介入と子の分岐（#306）

画面の「この時刻から分岐を作る」の`POST …/branches/{branch}/intervention-preview`と`intervention-apply`に、APIキーのないVMと同じ答えを返します（確かめ、答え、下書き、`last_fork`は中核の`sheltersim_core::answers::interventions`。ブラウザーは`native/wasm/js/interventions.mjs`がファイルとRunを読み、子を作ります。#449）。

- プレビューは、Branchのその分の世界から要約（`Simulation.scenarioSummary`、#302）を作り、文をAPIキーなしで読み取ります（`compileInterventionOffline`、#303）。案が`compiled`なら、そのEffectProgram（`effectProgramDump`、#304）を世界で確かめ（`validateEffectProgram`）、承認のhash（programの正規のJSONのsha256）と下書きの`preview_id`を足します。下書きはProjectのフォルダーの`drafts/<preview_id>.json`に置き、Projectの1つのファイル（bundle）には入れません。下書きに伏せるべき文（連絡先、鍵）があれば、serverと同じく409で断ります。
- 適用は、下書きと承認のhashを確かめ、serverのprocessのworkerと同じ`{"status": "running", "branch_id", "minute"}`を返します。子の分岐は答えの前に作ります。親をその分に戻し、プレビューが見た状態（`state_hash`）と同じことを確かめ、programを予定に入れ（`Simulation.applyEffectProgram`、#305）、子の保存したRunを書きます。結果は、serverと同じくRunの記録（`GET runs/{run}`）の`last_fork`（`{"status": "created", "branch_id", "minute", "state_hash"}`か`{"status": "failed", "error", "code"}`）で返します。子の記録は`paused`、`fork_minute`と`computed_until`は分岐の分です。続きは`resume`で計算します。
- 子の保存したRunは、形`sheltersim-run-v6`です（#368、#478）。`run`の行に`fork_minute`と`parent_branch_id`を持ち、その次の`fork`の行に分岐の元（`fork_origin`：親、分、分岐前の世界のdigest、親の先頭）、予定に入れたprogramの正規のJSON、承認のhash、受領（Pythonの`command_log`に足す記録）と、親の保存したRunのどこまでを読むか（`base`：親の目録の行の番号、その行の先頭のhash、分、親がその行までに保存した表・塊・LLMのテープの数）を持ちます。子は親の部分を写さず、`base`の行（分岐の世界を戻した親の途中の点。0分からなら`export`の行）まで親の部分を読みます（中核の`run_store::Chain`、#368の決定6）。子が持つのは、そこから分岐の分までのログ・標本と、programを予定に入れた後の分岐の分の途中の点、その後の計算です。読むときは親の目録も読み、`base`の行が同じ先頭のhashで残っていることを確かめます。親を消すのはRunごとなので（`POST runs/{run}/delete`）、子だけが残ることはありません。分岐の分より前の分は、このRunの分ではありません（中核の`Chain::seek`が断ります）。

serverと違うのは次のところです。

| 欄・動き | ブラウザー |
|---|---|
| 型つきの`program`のプレビュー | 分の確かめの後、409の`browser_unsupported`（画面は送りません。pydanticの誤りの文は作りません） |
| `preview_id` | serverと同じく乱数（uuid4の16進） |
| 適用の間のRunの状態 | 子は答えの前にできるので、Runが`running`になることはありません。ほかの分岐の計算中も、serverの2つのworkerの数には数えません |
| `last_fork` | serverと同じく、そのページのWorkerが覚えます。ページを閉じると消えます |

`native/wasm/tests/node_interventions.mjs`が、`native/core/tests/data/interventions.json`（`tests/support/core_interventions.py`が、processのworkerのserverで作ります）のRunで、プレビュー（読み取れる文、あいまいな文、読めない文、範囲の外、伏せる文、serverの誤り）、適用とその誤り、`last_fork`のあるRunの記録、最後まで計算した子のframeと指標の時系列が、serverと同じbytesになることを確かめます（`preview_id`と各Branchの先頭は伏せます）。

## ブラウザーの比較と質問（#307、#308）

`GET runs/{run}/compare?branch_ids=&minute=`と2つの分岐の`GET runs/{run}/replay-frames`は、serverと同じ比較を返します（中核の`sheltersim_core::answers::frames`。ブラウザーは`native/wasm/js/comparison.mjs`がjournalとframeを読みます。#449）。各分岐のその分のframe、共通の計算済みの範囲、各分岐の分岐の受領（`fork_origin`。子の保存したRunの`fork`の行から読みます。分岐で作っていない分岐は`null`）、同じ元を持つか、最初の分岐からの指標の差です。差は中核が求めます（`kpiDifferences`）。整数どうしの差は整数、小数を含む差は小数で、Pythonと同じ書き方になります。受領の`checkpoint_hash`は、子を作ったときの親の先頭（目録の行のsha256）で、serverのCheckpointのhashではありません。

`POST …/branches/{branch}/questions`は、APIキーのないVMと同じ答えを返します（中核の`sheltersim_core::answers::questions`。ブラウザーは`native/wasm/js/questions.mjs`が記録・Scenario・frameを読み、乱数と時刻を渡し、turnを残します。#449）。serverと同じ順に、要求の欄、質問の長さ、分、人、比べる分岐（計算が終わった分岐か、その分のframeにその人がいるか、比較の確かめ）を確かめ、「Claude APIが利用できないため、結果に関する質問へ回答できません。…」と、質問の文脈（`context`）とそのid（`context_id`）を返します。idは、文脈の正規のJSONのsha256の先頭32桁と24桁の乱数で、要求が同じ文脈のidを渡したときはそれを使います。会話はBranchの保存したRunに、`end`の行の後の`conversations`の部分として1つずつ残し（#368。Branchの先頭は変わりません）、質問はserverと同じく連絡先と人などの名前を伏せた文にします（中核の`questionText`）。`history_query`（保存した履歴を根拠にする質問）は#376でなくし、serverもブラウザーも知らない欄として断ります。

`native/wasm/tests/node_interventions.mjs`が、同じRunで、比較と2つの分岐の再生（受領の`checkpoint_hash`は伏せます）と、質問の答え（`context_id`の乱数の部分は伏せます）と残した質問が、serverと同じになることを確かめます。

## ブラウザーのPDFレポート（#309）

`POST …/runs/{run}/pdf-report`には、APIキーのないVMと同じ答えを返します（中核の`sheltersim_core::answers::routes::pdf_report_answer`。ブラウザーは`native/wasm/js/browser_api.mjs`の`pdfReport`から呼びます。#449）。serverと同じ順に、欄（`branch_ids`と`minute`だけ）、範囲（1つか2つの異なるBranchと0以上の整数分。違えば409の`pdf_invalid_scope`）、Run、各Branch、計算済みの範囲（外なら409の`pdf_uncomputed_range`）を確かめ、通れば409の`pdf_analysis_offline`（「Opusの記録応答またはAPI接続が必要です。」）を返します。APIキーのないVMも分析にOpusが要るのでPDFを作りません。ブラウザーはPDFを作りません。

serverと違うのは、`minute`を`5.0`のように小数の形で書いたときだけです。serverは整数でないとして`pdf_invalid_scope`にしますが、JSのJSONは`5.0`と`5`を区別しないので、ブラウザーは5分として扱います。

読めないJSONの本文には、serverと同じく400の`{"error":"invalid project request"}`を返します（`code`の欄はありません）。

`native/wasm/tests/node_api_answers.mjs`が1つの分岐の確かめを、`node_interventions.mjs`が子と2つの分岐（共通の範囲の外を含む）の確かめを、serverの答えと比べます。
