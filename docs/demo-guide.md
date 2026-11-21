# ローカルデモ

画面は`/`の1つだけです（ファイル名は`demo-v2*`のまま。[操作・API対応・検証](ui-backup.md)）。審査員などに共有のVMで使ってもらう手順は[vm-deploy.md](vm-deploy.md)にあります。サーバーなしでブラウザーだけで動く版（計算済みの例を開き、分へ飛ぶ・再生する・時系列を見る。計算や介入、PDFはまだ未対応）は、静的サイトとして作れます（[development.md](development.md#ブラウザー版の静的サイト266)）。

## アプリの起動

開発依存とRustのkernel（[開発環境](development.md#rustのkernel)）を入れ、repository rootで次を実行します。kernelは必須です。

~~~shell
python -m pip install -e ".[dev]"
SHELTERSIM_LLM_MODE=offline sheltersim web \
  --runs-dir /tmp/sheltersim/runs \
  --host 127.0.0.1 --port 8765
~~~

http://127.0.0.1:8765/ を開くと、ホームが出ます。保存したシナリオを開くか、新しいシナリオを作ります。`/index.html`は、クエリを保ったまま`/`へ302で送ります。前の道（`/demo-v2`、`/demo`、`/admin`）は転送しません（#434）。

Projectは`--runs-dir`の下の`.projects`に保存されます。計算と分岐は、Runごとに別プロセスで動きます。異なるRunは、既定で2つまで同時に計算できます。`--max-parallel-runs N` で変えられます。保存データ（Runの記録と保存したRun）は、`.projects`ごとに既定で10 GB（10^9バイト）までです。`--project-storage-gb N`で変えられます。計算1本で、1日分のRun（100人）なら約0.5〜0.9 GBのメモリを使うので、PCのメモリに合わせて決めてください。

このserverはローカル用途で、loginを持ちません。別途確認済みのaccess controlがない限りloopbackにbindしてください。

## デモデータ

同梱Scenarioと画面で作るScenarioは合成データです。Providerを設定していない場合、free-text previewはoffline parserを使います。previewだけでBranchは変更されません。介入は、分岐比較の画面で作用・対象・量（期間のある作用は続ける時間）を選んで組み立て、確認と明示承認の後に子Branchを作ります（#584）。介入を文章で指示するのは、API が ON のときだけ使える選択肢です。

## 計算済みの例

3つの例（それぞれ1つのRunと、介入した分岐の比較）を、画面が地図を描けるProjectとして作るには、`scripts/build_project_sample.py`を使います。出力先は、`sheltersim web`の`--runs-dir`の下の`.projects`です。

~~~shell
SHELTERSIM_LLM_MODE=offline python scripts/build_project_sample.py /tmp/sheltersim-sample/runs/.projects
SHELTERSIM_LLM_MODE=offline sheltersim web --runs-dir /tmp/sheltersim-sample/runs \
  --host 127.0.0.1 --port 8765
~~~

| Project | シナリオの文 | 分岐の介入 | 比較で見えること |
|---|---|---|---|
| `sample`（ホームの先頭） | 東京都港区で地震。避難者70人、7日間、面積700㎡ | 1日目18:00に、避難スペースへ30人が到着（帰宅困難者を想定）。合計100人 | 混み合いで、未対応の支援要請、強いストレスの人、トイレ待ち、水の減りが少しずつ増える |
| `sample-2` | 墨田区で地震。避難者100人、7日間 | 1日目12:00に、運営本部へスタッフを5人追加（6人→11人） | ストレスと未対応の支援要請が大きく下がる |
| `sample-3` | 江戸川区で水害。避難者100人、5日間 | 2日目08:00〜20:00に、食事・給水スペースで断水 | ストレスが急に上がり（20:00ごろが山）、その後下がる |

- どの例も、どの時点でも100人以下、7日以下です。例のProjectは`sample`と`sample-…`で、画面はこの名前のProjectを例として扱います（自分のシナリオはそこに作りません）。
- Scenarioは、それぞれの文を`scenario-preview`（offline parser）と`scenario-approve`に通して作ります（画面は#583から、文をAIでだけ読みます）。標準の合成施設、seed 2026です。地域の公開データの調べ（国土地理院・e-Statなどへの接続）は、結果が日によって変わるため行いません。Scenarioの記録（`provenance.region_research`）に、その旨と固定の日付が残ります。そのため、自治体と災害の種類はシナリオの言葉だけで、計算の結果を変えません（同じ人数なら、港区と墨田区の世界は同じに進みます）。到着する30人も、モデルの上では普通の避難者です。
- その後、`main`の最後までの計算、分岐の時刻での介入（分岐比較の画面が中核の作用の一覧から組み立てるのと同じ型付きのprogram、#584）のプレビューと明示承認、分岐の最後までの計算を、画面と同じProjectAPIの操作で順に行います。Providerは呼ばず、利用台帳と応答テープは一時directoryに置きます。そのProjectがすでにある出力先には作りません。
- 終わると、各例のScenario（文章、ID、hash、施設、トイレ）、Run ID、分岐、各分岐のworld・イベント・指標のhash、大きさを表示し、出力先の隣の`sample-build.json`（例では`/tmp/sheltersim-sample/runs/sample-build.json`）にも書きます。同じcommitなら、作り直してもhashは同じです。16スレッドのPCで、3つ合わせて約2分です。
- 同じserverの`/`のホームの「保存済みのシナリオ」に、「例」の付いた行が3つ出ます。表示される`open_path`でも開けます。例：http://127.0.0.1:8765/?project=sample&run=run-minato-arrivals&branch=main#simulation 。元のシナリオの1日目00:00から地図の上で再生でき、分岐比較の画面で「比較を更新」を押すと介入した分岐と比べられます。分岐は、シミュレーションの画面の分岐の選択でも開けます。
- 3つの例は、19通りの試しの計算から、比較の差が見やすいものを選びました。以前の1つの例（墨田区100人、4日目14:00にトイレ1基が故障）は、24時間平均のトイレ待ちが約0.5分変わるだけでした。24時間の断水は計算がとても遅くなるため（#604）、12時間にしています。断水の間に未対応の支援要請が0件近くまで下がる挙動は、モデルの確認を#605で頼んでいます。
- 計算済みの合成シナリオの例で、実際の避難所の記録や予測ではありません。1つのseedの比較は記述であり、介入の効果の根拠にはなりません。共有のVMでの使い方は[vm-deploy.md](vm-deploy.md)の「計算済みの例」にあります。

## CLIのRun

`sheltersim run`は、画面と同じ道でProjectにRunを作ります（既定は`runs/.projects`）。`sheltersim web`の`--runs-dir`をその親にすれば、画面で開けます（[API・CLI](api-reference.md)）。

## 困ったとき

- 8765番portが使用中なら別portを指定し、そのURLを開きます。
