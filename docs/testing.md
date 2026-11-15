# テストと検証

テストは、Scenarioの作成と検証、保存したRunの読み書きと保存した点からの再開、独立したBranch、中核の計算の結果（全経路のハッシュと記録した値）、モデルの出典、LLMのfallbackと応答テープ、Projectの保存、Webの経路、地域データの取り込みを確かめます。Rustの中核の規則は`cargo test`が確かめます。

この文書には、決まりと手順だけを書きます。テストは変わり続けるので、件数・網羅率・計測時間のように変わる数は載せません。これまでの分析と経緯は、git履歴とIssue（テストの整理は#53・#57・#126、網羅の確認は#176・#178）を見てください。

## 通常のoffline確認

開発toolとRustのkernel（[開発環境](development.md#rustのkernel)）を導入し、次を実行します。kernelがないと`sheltersim`を読み込めず、テストは動きません。

~~~shell
python -m pip install -e ".[dev]"
SHELTERSIM_LLM_MODE=offline python -m pytest -n auto
ruff check .
ruff format --check .
cargo fmt --manifest-path native/Cargo.toml --all -- --check
~~~

- `ruff check .`と`ruff format --check .`は指摘なしで通る状態を保ちます。1行100桁を超える文字列は、文言を変えずに、隣り合う文字列リテラルの連結に分けます。分けると値が変わる複数行の文字列（SQLiteの表定義など）だけ、閉じる引用符の行に`# noqa: E501`を付けます。
- テキストのファイルの読み書き（`Path.read_text`・`write_text`、`open`）と、`subprocess`の出力を文字列で受け取るとき（`text=True`）は、`encoding="utf-8"`を明示します。指定しないとOSの既定の文字コード（Windowsではcp1252やcp932）になり、日本語を読み書きできません。
- テストに固定したハッシュやダイジェストは、Linux・Windows・macOSで同じ値と比べます。OSで条件を分けません（`sys.platform`）。テストの入力を作るときも、OSの数学ライブラリに頼る関数（`math.cos`、`math.sin`、`math.exp`、`math.log`、float の`**`など）を使いません。最後のビットがOSで変わり、入力が変わるためです。乱数の`uniform`・`random`と四則演算、`round`、`repr`はOSによりません。
- Rust（`native/`）の書式は`native/rustfmt.toml`で決め（#367）、`cargo fmt --all -- --check`が差分なしで通る状態を保ちます。Rustを変えたら`cargo fmt --manifest-path native/Cargo.toml --all`で整形してからcommitします。
- 一括整形のcommitは`.git-blame-ignore-revs`に登録してあります。手元の`git blame`で整形前の変更をたどるには、一度だけ`git config blame.ignoreRevsFile .git-blame-ignore-revs`を設定します。GitHubのblame表示はこのファイルを自動で使います。
- 有料APIを呼ぶテスト（`live_api`の印）は、`SHELTERSIM_ALLOW_LIVE_API_TESTS`と`ANTHROPIC_API_KEY`の両方を明示した場合だけ実行されます。通常の実行とCIでは設定しません。
- `tests/conftest.py`は、テストごとにProjectの保存先（`SHELTERSIM_PROJECTS_DIR`）、状態の保存先（`XDG_STATE_HOME`）、LLMのcache（`SHELTERSIM_LLM_CACHE_PATH`）を一時ディレクトリに向けます。並列のworkerがこれらを共有せず、開発者のRunやPC全体の利用記録にも書き込みません。テストの中で設定した値が優先されます。`live_api`のテストは、実費をPC全体の利用記録に残すため対象外です。

### 並列実行と長いテスト

- `-n auto`（pytest-xdist）で並列に実行します。`pyproject.toml`の`--dist worksteal`で、手の空いたworkerが残りのテストを引き取ります。
- `long_simulation`の印は、7日間のsimulationを実行するテストと、1件で30秒以上（直列）かかるテストに付けます。`tests/conftest.py`の`pytest_collection_modifyitems`が、並列のときにこれらを各workerの塊の先頭に1本ずつ置くので、長いテストは最初から並んで走ります。
- 開発中は`-m "not long_simulation"`で長いテストを除けます。印のない`pytest`は全件を実行します。workflow`PRの確認`（`.github/workflows/ci.yml`、2026-11-15までの名前は`オフラインCI`）は、mainへのPull Requestごとに自動で、または手動で、Linux・Windows・macOSの3つでRustのkernelを作って導入し、テストを`-n auto`で実行します。3つのOSで動くことを確かめるためです（#48）。Linuxは、1つのjob（`テスト全件と計算結果（Linux）`）で長いテストを除いた全件を実行します。以前はテストを2つのjobに分けていましたが（#118）、Pythonのシミュレーションを消してテストが短くなり、kernelを作る時間のほうが長くなったので、1つにまとめました（#477）。WindowsとmacOSは、それぞれ1つのjobで、OSによって振る舞いが変わりうるテストだけを実行します（`--os-subset`、#380）。`tests/unit`と`tests/e2e`のすべてと、`tests/integration`のうち印`os_dependent`を付けたファイルです。Runのbytesは全経路のハッシュで3つのOSで同じと確かめるので、計算の結果だけを確かめるテストはLinuxで足ります。ファイルの書込み・置換え・削除やロック、プロセスの起動、OSを通した計測（メモリーなど）、CPythonの数の関数（OSのlibm、Windowsで32 bitのC long）を確かめる`tests/integration`のファイルには、`pytestmark = pytest.mark.os_dependent`と、その理由のコメントを付けます。審査に出す前や公開の前などには、手動実行の`全件の確認（手動・3つのOS）`の`full`で、3つのOSで全件を実行します。テストのjobは、kernelを`native/Cargo.toml`のprofile`ci`で作ります。releaseと最適化の水準は同じで、LTOをthinに、codegen unitを16にして、作る時間を短くしたものです。Runのbytesは変わりません（#217）。全経路のハッシュ（`benchmarks/gate_all_paths.py --expect`）も3つのOSで同じ`fixtures/gate-all-paths.json`と比べます。どのOSでも、テストの後の手順として、同じprofile`ci`のkernelで動かします（テストが失敗しても動きます）。配布と同じ`--release`で作ったkernelは、手動実行の`全件の確認（手動・3つのOS）`で確かめます（#477）。lint・format・clippyはLinuxだけの別のjob（`書式・規則`）です。formatはPythonの`ruff format --check .`とRustの`cargo fmt --all -- --check`の両方を確かめます（#367）。その隣の別のjob（`Rustの計算部分のテスト`）で、Rustの中核（`native/core`、#152）がpyo3に依存しないことを確かめ、その`cargo test`と、保存したRunのファイルの置き場（`native/store`、#426）の`cargo test`を実行します。中核の`cargo test`は、`native/Cargo.toml`のprofile`ci-test`で作ります。devの検査（overflow、`debug_assert`）を残したままopt-level 1にしたもので、テストの中のシミュレーションが速く進みます（#380）。`native/core/tests`のファイルはそれぞれ別のバイナリになるので、LTOを使うprofile（`ci`、`release`）では、リンクに時間がかかりすぎます。もう1つの別のjob（`ブラウザー版の計算`）で、中核を既定の機能`parallel`（rayon）なしで`wasm32-unknown-unknown`向けに作り、`parallel`なしの`cargo test`も実行します。機能が違うと中核のテストを作り直すので、`Rustの計算部分のテスト`と並べて実行します（#380）。手動実行の`全件の確認（手動・3つのOS）`（`.github/workflows/long-regression.yml`）の`long`は長いテストだけを、`full`は全件を、3つのOSで実行します。`PRの確認`と同じく、Rustのkernelを作って導入してから、`-n auto`で実行します。

### 結果の残り方と見方

pytestを実行するたびに、`tests/results_log.py`（`tests/conftest.py`が読み込むplugin）が、結果をリポジトリ直下の`.test-results/<UTCの日時>-<プロセスID>/`に書きます。`.test-results/`はgitに登録しません。全件でも一部でも、並列でも同じです。

- `summary.txt`：実行したコマンド、件数、失敗（FAILED）とエラー（ERROR）の一つ一つについてtracebackと捕まえた出力、時間のかかったテスト15件。実行の終わりに書きます。
- `results.jsonl`：テストの段階（setup・call・teardown）ごとに1行で、結果、時間、並列のworker、失敗ならtraceback。実行中に書き足すので、途中で止まった実行でもそこまでの結果が残ります。
- `.test-results/latest.txt`：いちばん新しい実行のディレクトリ名。ディレクトリは新しいものから20回分だけ残します。

出力の最後の行は、pytestの集計行のあとに足す次の1行です。失敗があれば、失敗したテストの名前（5件まで）と`summary.txt`の場所が入ります。

~~~text
test results: 1 failed, 967 passed, 4 skipped; failed: tests/unit/x/test_y.py::test_z; tracebacks in .test-results/20261028T010203Z-1234/summary.txt
~~~

失敗の原因は、もう一度実行せずに`summary.txt`で確かめます。`.pytest_cache`の失敗記録（`--lf`）は、次に全件が通ると消えるので、記録としては使いません。コーディングエージェントも、`| tail -1`などで集計行だけを見て判断しないでください。失敗があったら、この最後の行で名前を、`summary.txt`で原因を確かめ、テストの名前と原因を報告します。結果を残したくない実行では`-p no:results_log`を付けます。

`PRの確認`は、jobごとの`.test-results/`を`test-results-<OS>-<1か2かos>`のartifactとして7日間保存します。失敗したときは、そのartifactの`summary.txt`で原因を確かめます。WindowsとmacOSの全経路のハッシュは、`gate-<OS>`のartifact（`summary.json`）に残ります。

### 網羅率の計測

網羅率（行と分岐）は、テストの届いていない処理を探すときだけ測ります。開発依存にpytest-covとcoverageを入れてあり、設定は`pyproject.toml`の`[tool.coverage.*]`です（対象は`sheltersim`、分岐も数える）。通常のpytestは計測しません。

~~~shell
SHELTERSIM_LLM_MODE=offline python -m pytest -n auto -m "not long_simulation" --cov --cov-report=html
python -m coverage report --sort=cover
~~~

- 結果はリポジトリ直下の`.coverage`と`htmlcov/`に書かれます（どちらも`.gitignore`の対象）。`htmlcov/index.html`で、ファイルごとに通っていない行と分岐が見えます。
- `long_simulation`の印のテストは外します。計測すると極端に遅くなる（全件で2時間を超える）一方、外して通らなくなる行はわずかです。
- 数字を上げること自体は目的にしません。通っていない処理のうち、利用者の操作から届くものを探し、その経路でテストを足します。

## テストの置き場所と名前

テストは範囲ごとにディレクトリを分けます。範囲は、1つのテストが通る一番広い範囲で決めます。

| 置き場所 | 範囲 | 名前 |
|---|---|---|
| `tests/unit/<担当>/` | 単体：中核のRunで時間を進めず、HTTP・CLIも通さない | `test_<srcのモジュール名>.py`。例：`src/sheltersim/experiments/profiles.py`なら`tests/unit/experiments/test_profiles.py` |
| `tests/integration/` | 統合：中核のRun（`PlainRun`、保存したRunの置き場）で時間を進めるか、HTTP・CLIを通す（どちらか一方） | `test_<機能>.py`。例：`test_plain_run.py`、`test_run_files.py` |
| `tests/e2e/` | e2e：HTTPかCLIから入り、時間も進める | `test_<利用者の流れ>.py`。例：`test_project_scenario_to_results.py` |
| `tests/live/` | 有料APIを呼ぶテスト（`live_api`の印） | `test_live_api.py`だけ。APIキーを使えるようにするゲート（`sheltersim.llm.config`）が、このパスのテストにだけ開くため |

- `<担当>`は、AGENTS.mdのモジュールのうちPythonに残るもの（`experiments`、`policies`、`region`、`llm`、`storage`、`web`など）と`cli`、それにリポジトリ自体（テストの配置、文書、fixture、出典台帳）を確かめる`repo`です。
- 名前に、不具合名（例：`resource_loss_without_operational_state`）、開発の回の名前（例：`round2_common_services`）、`regressions`を使いません。不具合の再発防止テストはその機能のファイルに足し、守るべき振る舞いを関数名に書き、Issue番号をdocstringかコメントに書きます。
- ブラウザー用の静的ファイル（JavaScript、HTML）を確かめる単体は、Pythonのモジュール名がないので`tests/unit/web/test_static_<ファイル名>.py`にします。例：`demo-v2.js`なら`test_static_demo_v2.py`。nodeで関数を動かすテストは、nodeがない環境ではskipします。
- 何を確かめるか（結果・契約・画面）は、ディレクトリにも印にもしません。実行の選び分けに使わない印は、付け忘れても気づけず、すぐ実態とずれるためです。確かめる振る舞いは、関数名とファイルのdocstringに書きます。
- 背景で動くjob（`ProjectAPI`）の終わりは、実時間の短い期限で状態を読み続けて待ちません。CPUが混むと期限を超えて失敗するためです（#143）。workerが1つなので、`api.wait_idle(timeout=120)`で、それまでのjobが終わるのを待ってから状態を確かめます。`timeout`は止まったときにテストを終わらせるためだけのものです。
- `tests/unit/repo/test_test_layout.py`は、`tests/`直下のテストファイル、決まった場所以外のテストファイル、`tests/live/`以外での`live_api`の印を見つけると失敗します。`scripts/`のファイルに書かれたテストファイルのパスが存在しないときも失敗します（#146）。
- 件数が要るときは、`pytest --collect-only -q <ディレクトリ>`で数えます。

### 共有する準備の置き場所

同じ名前のファイルを別のディレクトリに置けるよう、pytestは`--import-mode=importlib`で動かします（`pyproject.toml`）。テストファイル同士ではimportできないので、共有する準備は`tests/support/`に題材ごとのモジュールとして置き、`tests/conftest.py`の`pytest_plugins`で読み込みます（#138）。`pyproject.toml`の`pythonpath`に`tests`を入れているので、`support.<モジュール名>`として読み込めます。

- 状態や後片付けがある準備（保存先、API、偽のサービス、保存済みRunなど）は、fixtureにします。テストファイルはfixtureとして受け取ります。
- 状態を持たないものは、テストファイルから直接importしてかまいません。`fixtures/`のファイルのパス（`support/paths.py`の`SCENARIO_100_AGENTS`など。テストの中で`Path(__file__).parents[N]`から組み立てません）と、合成データの定数とそれを作る関数（`support/public_data.py`の`MUNICIPALITY`など）です。
- `tests/conftest.py`に残すのは、すべてのテストに効く設定（ローカルの状態の隔離、`--os-subset`による選り分け、並列実行のときの長いテストの並べ替え）と、結果を残すplugin `tests/results_log.py`の読み込みだけです。
- 新しい準備は、題材が合うモジュールに足します。合うモジュールがなければ、新しいモジュールを作って`pytest_plugins`に加え、下の表にも足します。

| モジュール | 題材 | 主なfixture |
|---|---|---|
| `support/generated.py` | UIで作る生成ScenarioとProject画面のAPI | `generated_scenario`、`generated_scenario_with`、`generated_previews`、`ui_project_api`、`generated_request`、`place_three_at_facility`、`add_toilets_program` |
| `support/fixture_scenarios.py` | `fixtures/`のScenarioを基に、1つの仕組みを確かめるために組み立てたScenario | `small_replay_scenario`、`small_benchmark_scenario`、`resource_service_scenario`、`information_spread_scenario`、`research_psychology_scenario` |
| `support/storage.py` | Projectの保存、保存したRun（形v6）、ほかのOSでの保存 | `project_store_at`、`saved_project`、`windows_like` |
| `support/web.py` | Projectの保存先を持つ、socketのないrequest handler | `bare_request_handler` |
| `support/llm.py` | 偽のLLM Providerと、有料APIのライブの確認 | `counting_provider`、`recording_router`、`live_model_check`、`live_source_search_check` |
| `support/reports.py` | PDFレポートの根拠つきの文章と、それを答える偽のOpus | `prose_document`、`report_provider` |
| `support/public_data.py` | 地域の調べが呼ぶ公開サービス（国土地理院の住所検索・指定緊急避難場所、e-Stat、気象庁の予報、設定した地域の公開データ）の偽物。存在しない自治体（合成県合成市）の合成の記録を返し、`fail()`で接続の失敗を起こせる。ネットワークには出ない | `public_data`、`researching_project_api`、`research_request` |

`support/paths.py`（`fixtures/`のファイルのパス）、`support/isolation.py`（隔離の処理）、`support/core_fixtures.py`（中核のテストのfixtureのうち、今も書き直せるものの一覧。下の「中核のテストのfixture」）、`support/initial_corpus.py`（中核の0分の世界の作り方を確かめるScenarioのコーパス`fixtures/initial_corpus.json.xz`を読む部品）、`support/scenario_corpus.py`（画面のシナリオ作りの答えのコーパス）は、fixtureを持たない部品です。テストで使う`fixtures/`のファイルが増えたら、`support/paths.py`に定数を足します。

関数を返すfixtureは、呼ぶたびに新しいものを作るだけで状態を持ちません。そのうち`saved_project`、`add_toilets_program`、`recording_router`はsession単位にして、module単位のfixtureからも使えるようにしています。テストファイルの中のhelperがこれらを使うときは、テストが受け取ったfixtureを引数で渡します。

### 中核のテストのfixture

`native/core/tests/`（`cargo test -p sheltersim_core`）の規則のテストは、`native/core/tests/data/`のファイルの世界と期待する値を読みます（#225）。中核のcrateの`sheltersim_core::load`（`native/core/src/load/`）が世界を読みます。読む処理はブラウザーのHostも使うので（#244）、テストの中には置きません。

- ほとんどのファイルは、#430より前にPythonが作った世界（`export_world`の書き出し）と、Pythonの規則がその上で出した値を記録したものです。Pythonの世界と規則は#430で消したので、これらのファイルを書き直すものはありません。記録した値が、中核の答えを固定します。中核の答えを意図して変えたときは、理由をPull Requestに書いて、ファイルの期待する値を中核の値で書き直します。
- 今も書き直せるファイル（画面の経路のserverの答え、Pythonに残るScenario作りの答え）は、`tests/support/core_fixtures.py`の`FIXTURES`に、ファイル名と、それを作る関数を書きます。`python benchmarks/core_fixtures.py [名前 ...]`で書き直します。名前を省くと、すべて書き直します。`tests/integration/test_core_fixtures.py`は、登録したファイルごとに、今の関数が作るものと同じであることを確かめます。
- 中核の0分の世界の作り方（`sheltersim_core::initial`）は、`fixtures/initial_corpus.json.xz`のScenarioごとに、記録したdigest（Pythonが作っていたbytesのもの）と比べます（`tests/integration/test_initial_corpus.py`）。
- ファイルは小さく保ちます。小さな生成Scenario（数人）を使い、テストのファイルの間で世界を共有します。
- `sheltersim_core::load`は、信頼しない入力を読む前提で書きます。書き出しの形でない値は`Error`にし、crateのコードで`panic!`や`unwrap`をしません。形の誤りが誤りになることは`native/core/tests/load_input.rs`で確かめます。
- Rustのテストは、`load::entry(ファイル名, 世界の名前)`で書き出しを読み、`load::loaded`で世界とRunとその方策を、`load::host`で節と図面（`Geo`）を持つ`TestHost`を作ります。tickを動かすテストは`load::following_host`で`TestHost`を作ります。Runの途中の世界が要るテストは、中核でtickを進めて作れます（`support/ticks.rs`の`run`）。
- Pythonのないホストは中核のcrateの`plain::PlainHost`です。ローカル版・VM版・ブラウザー版のどれもこれで計算します（#427）。

## 利用者の経路を通る共通fixture

UIで作るScenarioは、必ず図面（`facility_geometry`）と認知（`agent_dynamics`）を持ちます。一方、`fixtures/`のScenarioはどちらも持ちません。#51の不具合は図面と認知がある場合にだけ起き、fixtureを使う多くのテストをすり抜けました。新しいテストは、特に理由がなければ`tests/support/generated.py`の共通fixtureを使います。生成器が作らない形（1つの施設に複数の列、迂回経路など）が要るときは、生成された図面に手で足すか、理由を書いてfixtureのScenarioを使います。

- `generated_scenario`：`ScenarioSpec`をテストごとに新しく作って返します。テスト内で変更しても、ほかのテストには影響しません。
- `generated_scenario_preview`：元になったpreviewの結果（`scenario`、`approved_hash`、`can_start`、`request`など）です。session単位で1回だけ作るので、変更してはいけません。module単位のfixtureから使うときは、`copy.deepcopy(generated_scenario_preview["scenario"])`から`ScenarioSpec`を作ります。
- `generated_scenario_with(**fields)`：画面の入力欄の一部を変えたときのScenarioを、テストごとに新しく作って返します。たとえば`generated_scenario_with(duration_days=4)`は、期間の欄に4と入力した場合です。変えた欄は利用者の入力として扱います。画面にない`InitialScenarioRequest`の項目（`seed`、`operational_conditions.scheduled`、`population_conditions`など）も渡せます。
- `generated_previews(**fields)`：上のpreviewの結果を返します。session単位で、同じ入力のpreviewは1回だけ作って共有するので、変更してはいけません。
- `ui_project_api`と`generated_request(**fields)`：作ったpreviewにさらに画面の操作（承認など）を続けるテスト用です。`ui_project_api`は、空の地域調査に差し替えた`ProjectAPI`（project名`project`）をテストの間だけ開いておきます。地域調査そのものを通すときは、`support/public_data.py`の`researching_project_api`を使います。

共通fixtureは、demo-v2のScenario画面が送る入力（`GENERATED_SCENARIO_REQUEST`：合成市・地震・20人・0.25日。各項目は利用者の入力として扱います）を、UIと同じ`ProjectAPI`のpreviewに渡して作ります。違いは地域調査だけで、本物は公開Webサービスに接続するため、空の調査結果に差し替えています。

この共通fixtureの実行結果は、`fixtures/regression-contract.json`の`generated_small`が固定していて、`tests/e2e/test_project_scenario_to_results.py`が確かめます（fixtureそのものは`tests/unit/repo/test_generated_scenario_fixture.py`）。

## 全経路の確認（ゲート）

7日分のハッシュの確認は、生成Scenarioが通る処理しか確かめません（メッセージや信念、介入、記録した応答などを通りません）。`benchmarks/gate_all_paths.py`で、そこを通る小さなofflineのScenarioの結果も確かめます。どのScenarioも中核（`PlainRun`）で計算します（#430）。

~~~shell
python benchmarks/gate_all_paths.py --output 出力先 --expect fixtures/gate-all-paths.json
python benchmarks/gate_all_paths.py --output 出力先 --determinism
~~~

- 各Scenarioは自分のプロセスで動き、終わりの世界ごとに中核のハッシュ（世界の部分ごとのdigestから作るworld、イベント記録、指標の標本）と時間を`出力先/summary.json`に書きます。ネットワークへの接続は失敗させます。LLMの応答は、手元の台本のProviderが書いた応答テープとその再生だけを使います。
- `--expect`：`fixtures/gate-all-paths.json`のハッシュと比べます。モデルや規則を意図して変えてハッシュが変わったときは、理由をPull Requestに書いて、`--write-expected`で更新します。これは挙動の変更で、リファクタリングと同じPull Requestには入れません（下の「挙動を変えない変更（リファクタリング）」）。理由が説明できない変化は不具合として調べます。この値はLinux・Windows・macOSで同じです。CIは3つのOSでこの値と比べます。OSによって変わったときは、OSの数学ライブラリ（libm）やCの関数に頼る計算が入っていないかを調べます。
- `--determinism`：別のプロセス、別の`PYTHONHASHSEED`、別の`RAYON_NUM_THREADS`（1と4）で、すべてのScenarioを動かして、1回目と比べます。`RAYON_NUM_THREADS`は、tickの人ごとの計算を手伝うスレッド（`native/core/src/parallel.rs`）の数を変えます。1は手伝いなし、4は手伝い3つです。人ごとの計算は少ない人数では呼んだスレッドだけで行うので、4の回は`SHELTERSIM_GATE_PEOPLE_PER_HELPER=1`で、どの人数でも1人ずつ手伝いに渡します。
- 保存した点からの再開、再開した Run のさらに再開、保存したRunの続きの計算は、1つのScenarioの中でまっすぐのRunと比べます（`must_match`）。
- 介入は、中核が予定できる操作（文章の読み取りが作る操作）なら、Runの途中の分に適用します。それ以外の操作は、0分の世界に予定した記録（`fixtures/gate-scheduled-programs.json.xz`）から計算します。この記録は、Pythonの経路を消す前に作り、Pythonの経路と中核が同じ世界で終わることを確かめたものです。
- `policies`のモデルを足したら、ここにもScenarioを足します。`tests/integration/test_all_paths_gate.py`は、モデルの一覧がここに入っていないと失敗します。
- 介入が拒否された結果と、有効な入力でRunが止まる結果（`run_stops`のScenario）も、期待値に入れます。直したら期待値が変わるので気づけます。止まるものは、見つけたときにIssueにします。直したものは`run_stops`にRunとして残し、再発に気づけるようにします。
- 全件を動かすテストには`long_simulation`の印を付けます。通常のpytestは、1つの小さなScenarioだけを動かします。

## 挙動を変えない変更（リファクタリング）

名前の付け替え、ファイルの移動、型の置き換え、関数の分割のように、挙動を変えないための変更（リファクタリング）は、次の条件をすべて満たします。満たさない変更は挙動の変更です。リファクタリングとは別のPull Requestに分け、何がなぜ変わったかを書きます（#487）。

- 全経路のハッシュが変わりません。`python benchmarks/gate_all_paths.py --output 出力先 --expect fixtures/gate-all-paths.json`を手元で通します。`--write-expected`で期待値を書き直さず、モデル全体の版`ENGINE_VERSION`（[architecture.md](architecture.md)）も上げません。
- 全経路のハッシュが見ない出力（Projectの道の答え、保存の置き場とProjectの束、ブラウザー版のWASMとJSの口、文章の読み取り、検証の誤りの文、0分の世界、LLMへの問い合わせなど）を固定したテストが、期待値を変えずに通ります。中核のテストのfixture（`native/core/tests/data/`、`benchmarks/core_fixtures.py`で書き直すものも）、`fixtures/`の記録した値、テストに書いたsha256やcache keyを書き直しません。書き直しが要るなら、それは挙動の変更です。
- 手元では、Pythonのoffline全件、Rustを触ったら`cargo test`（`sheltersim_core`は既定の機能と`--no-default-features`の両方、`sheltersim_store`）、ブラウザー版のWASMかJSを触ったら`native/wasm/tests/`のNodeのテスト（CIの`ブラウザー版の計算`と同じもの）を通します。
- 固定されていない出力を変えそうなリファクタリングは、先にその出力のsha256を固定するテストを足し（別のPull Request）、それが通ることを確かめてから変えます。小さな合成の入力で作り、期待値はLinux・Windows・macOSで同じものにします（上の「通常のoffline確認」）。

### 画面の整理の確かめ

画面のファイル（`src/sheltersim/web/static/`）のリファクタリング（たとえば`demo-v2.js`を分ける）は、ハッシュでは確かめられません。変える前のcommitと変えた後のcommitで同じ手順を行い、結果を比べます。

1. `tests/unit/web/`の`test_static_*.py`と、`tests/integration/test_static_site.py`・`test_demo_v2_routes.py`が通ります。
2. 静的サイト（[開発環境](development.md#ブラウザー版の静的サイト266)の「ブラウザー版の静的サイト」）を、それぞれのcommitで同じ例のファイルと避難所のCSVから作り、`benchmarks/static_site_e2e.py --site … --work … --python-projects … --require-all --output …`を流します。画面の操作を最初から最後まで通し、ブラウザーの答えをPythonのserverと比べる道具です。前後のどちらも、すべての段がすべてのbrowserで通り、結果のJSONの各段の確かめ（`checks`の`name`と`verdict`）の並びが同じになります。
3. 2と同じ例のProjectStore（`scripts/build_project_sample.py`の出力）の写しを`--runs-dir`の`.projects`に置いてローカル版のserverを起動し（`python -m sheltersim web --runs-dir … --port 8878`）、`scripts/review_demo_v2_gameplay.py --project sample --run run-sample-toilet-outage --branch branch-toilet01-outage --output …`を流します。前後の`recorded-frames.json`が同じbytesで、`results.json`の`recorded_audit`が同じになります。再生の速さやFPSのような実時間の値は比べず、道具の中の確かめが通ることだけを見ます。
4. 画面の見た目は、2と3が書く画面の写しを前後で並べて見比べます。描画の時刻で画素が変わるため、写しのbytesは比べません。

## 不具合を見つけたとき

テストで不具合が見つかったら、テストで隠さずにIssueにします（例：#65）。

- 不具合で今は確かめられない振る舞いは、期待する形でテストを書き、`pytest.mark.xfail(strict=True)`にIssue番号を付けて置きます。失敗の種類が決まっているときは`raises=…`で限ります。不具合を直したのに期待と違う結果になった場合も気づけます。直したら印を外します。
- テストの前提（その時刻にトイレが使われている、など）が崩れたときは、`assert`ではなく`pytest.fail`で失敗させます。`xfail`の印が前提の崩れを隠さないようにするためです。
- 外部から来る入力（LLMの出力、公開データ）の拒否は、理由付きで拒否されること（`StorageError`や契約の検証エラー）を確かめます。ほかの例外で落ちることを見逃さないためです。

## 出典台帳と根拠IDの整合

AGENTS.mdは、新しい係数の根拠を`fixtures/source-manifest.json`に記録するよう求めています。`tests/unit/repo/test_source_manifest.py`は、`src/`に固定の文字列で書かれた仮定ID（`ASM-…`）と出典ID（`SRC-…`）を集め、台帳にも既知の不足の一覧にもないIDがあると失敗します。

- 実行時に組み立てるID（`SRC-PLAN-<digest>`、`ASM-POLICY-OVERRIDE-<hash>`など）と、`str.startswith`に渡す接頭辞は、IDとして扱いません。
- 既知の不足は、テストファイルに分類ごとの一覧があります（#61）：別のファイルに記載があるが台帳にない仮定ID、どこにも記載がない仮定ID、実行時に取得する公開データの出典ID、利用者が入力した値の出典ID。
- 新しい仮定IDを一覧に足してはいけません。コードと同じ変更で台帳に記録します。台帳に記録する中身（出典、単位、上限、仮定、適用範囲）はモデル担当が決めます。
- 台帳に記録されたID、コードが使わなくなったIDが一覧に残っていると、テストが失敗します。一覧は減る一方です。
- 実行時のデータ提供元と利用者入力の出典IDを台帳に載せるかは、まだ決まっていません。新しく足す場合は、その分類の一覧に加え、理由をPull Requestに書きます。

## 確認できていないこと

- 有料のProviderを呼ぶライブのテストは、通常の確認では実行しません。
- Windows・macOSの実機上での起動は確認していません。
- 30日間の性能結果は確立していません。過去の30人・30日試行は10分以内に完了しませんでした。
- 画面の操作のうち、すべての地図レイヤー、介護・会話・職員の組み合わせ、大規模人口を含む一連の操作は、ブラウザーで確かめていません。
- 地域人口、避難所、天候、医療、結果の照合データを用いた外部検証はできていません。
- 心理、健康、社会、容量、incidentの係数は実測校正されていません。合成Branchの差は、条件付き出力であり、予測や現実の因果効果ではありません。

再現可能な1日性能値と環境は[benchmarks.md](benchmarks.md)、モデルの仮定と出典の記録は[model-validation.md](model-validation.md)と`fixtures/source-manifest.json`を参照してください。
