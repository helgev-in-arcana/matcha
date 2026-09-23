# レンダリングインターフェース報告後の整理 — 2026-09-23

対象は `codex/render-interface` の native Scene 実装。今回の変更は記録・説明の訂正と診断テスト。
描画動作や公開フィールドは変更しない。

## 後回しにする項目

| 項目 | 分類と判断 |
|---|---|
| snapshot 依存 | 将来のインターフェース候補。Source に依存フラグを宣言し、再生成判断を renderer に任せる案を記録。今回は追加しない。 |
| sampler-policy | 将来の描画意味の指定。nearest/linear が実証済みの不足。関連候補として address mode、mip/LOD、blend policy も検討できるが、採用は未決定。今回は追加しない。 |
| ページ数・論理内容量・ページ容量 | renderer の配置・予算・計測の話。インターフェース本体の課題から分離し、追加最適化は後回し。 |
| 内容 ID より上流のキャッシュ寿命 | UI/provider の所有・キャッシュ実装の話。インターフェース本体の課題から分離し、追加検討は後回し。既存の弱参照による誤同一視の修正は保持。 |

snapshot フラグ案では、同じ資源 ID でも配置、opacity、描画順、mask、viewport、初期画像が変われば
背景が変わり得る。実装時は「実行済みフェーズの資源 ID が変わったか」より広い snapshot 版の判定が必要。
また、現在の「同じ ID は同じ内容」という約束と、依存フラグ付き Source の有効なキャッシュキー
（例えば SourceId と snapshot 版の組）をどう整合させるかを決める。今は方式を確定しない。

## wgpu の検証と Result

実際の依存は wgpu / wgpu-core 29.0.4。ネイティブ実装の処理は次の通り。

1. `copy_texture_to_texture` は Copy 命令を記録する。この形式不一致はまだ報告されない。
2. `encoder.finish()` が記録済み命令を encode し、RGBA16Float → RGBA8Unorm の Copy を拒否する。
3. wgpu は error scope があればそこへ、なければ uncaptured handler へエラーを渡す。
4. ハンドラが復帰すれば `finish()` は CommandBuffer を返す。API の戻り値は Result ではない。

このエラーは GPU 実行の失敗ではなく、CPU 側での GPU API 利用の検証エラー。
wgpu 標準の uncaptured handler は panic するが、Matcha の gpu-utils はログ出力に置き換えている。
元の負のテストは error scope が受け取るため、やはり panic せず、Source の `Ok(())` とも別経路になる。
SceneRenderer の Result はこの経路を取り込んでいないため、検証エラーがあっても Ok になった。

`scope.pop()` の公開 API は Future を返すが、このネイティブ実装は保存済みエラーを
`ready(scope.error)` で返す。その型だけで検証が非同期だったとは言えない。
他の検証すべてが finish 時とは限らず、資源作成時や submit 時などにも検証点がある。
ブラウザのエラー配送や device-loss、実行完了の保証は今回の確認範囲とは別。

追加テスト `diagnostic_gpu_validation_at_finish_does_not_wait_for_execution` は handler の呼出数を確認する。
Copy 呼出後 0、finish が戻った直後 1。queue.submit / device.poll を行わず Vulkan / DX12 で成功した。
元の CPU Result と scope の相違を確認するテストも両 backend で再実行し、各 2 件成功。
したがって報告中の「非同期観測口が必要」という結論は撤回し、まず renderer のエラー方針の話とする。

```powershell
$env:MATCHA_TEST_BACKEND='vulkan' # 次に dx12 でも実行
cargo test -p renderer --test scene_contract --offline -j 2 diagnostic_gpu_validation -- --nocapture --test-threads=1
```

## 独立したローカル Scene とは

「各ウィジェットが別々に所有・更新する CPU の Scene」という意味であり、
個別の offscreen 描画先・背景・合成グループを持つという意味ではない。
これは現在の UI フレームワークの構成であって、renderer が複数 Scene を要求する契約ではない。

各 RenderItem がローカル Scene を保持し、GuiRenderer が一つの Scene に平坦化して渡す。
`append_scene` が行うのは、資源定義の import、Object/PixelMask の配置変換、mask index の付け替え、
祖先 clip の継承、各 Object の opacity の乗算、同じ番号の Phase への Object 追記。
資源 ID は書き換えない。同一 ID の import は descriptor の一致を確認し、内容同一性は提供側の約束に委ねる。

例えば A と B がともに Phase 0 に通常描画、Phase 1 に背景効果を持ち、A の後に B を追加すると、
全体の順番は `A0 → B0 →（ここまでが次の snapshot）→ A1 → B1`。
Phase 1 の生成は Phase 1 の描画より前に行うので、B1 用の生成も A1 の結果は見ない。
これは `A0 → A1 → B0 → B1` ではなく、A の Phase 1 は B の Phase 0 も背景として見る。

さらに、配置行列を Object に掛けても Source のクロージャが捕捉した値は書き換わらない。
背景の座標は画面全体に対するものなので、背景効果を構築するときには RenderCtx の
解決済み transform / viewport を使う。通常の画像・glyph 等は配置と独立して共有できる。
複数 Object に opacity を掛ける動作も、先に全体を一枚に描いて group opacity を掛ける動作とは異なる。

独立した CPU キャッシュを共有するため、現在の Source::clone は Arc<Prepare> を共有する。
中央の ResourcePool に定義を集約して、各ウィジェットには描画レコードと ID だけを持たせる構成も可能。
Arc の採用やこの所有構造自体をインターフェースの必須要件とはしない。

設計上の論点は「複数 Scene を renderer に渡せない」ことではなく、再利用部品としての Scene に
どこまで局所性を期待するか。現状の平坦化には明確な意味があり、それで十分なら追加機能は不要。
局所フェーズ・局所 snapshot・隔離された group opacity を保証したい場合は、別の合成規則や
中間画像を使う実装を検討する。今回は課題の説明までとし、そうした保証を追加していない。
