/* F18 比对工具：把任意 `.inf` 容器的段表按 `infy_art_dump_meta()` 打到 stdout。
 *
 * 为什么单独一个 main：`infy artifact sections`（Rust 侧）与 C 侧渲染此前只在 rt 夹具上比过，
 * 而 F18 的验收要求「同一夹具逐行可比」——真产物（examples/tour 的 ci.exe）也要能比。
 * rt harness 只 dump 它自己造的那几份文件，所以这里给脚本一个入口。
 * 不进 CI 的任何编译行，只由 scripts/f18-dump-compare.sh 自己编译。
 *
 * 用法：art_dump <file.inf>   （拒绝时 infy_art_read_file 自己把 why 打到 stderr，这里只返退码）
 */
#include "infy_rt.h"

#include <stdio.h>
#include <stdlib.h>

int main(int argc, char** argv) {
    if (argc != 2) {
        fprintf(stderr, "usage: art_dump <file.inf>\n");
        return 2;
    }
    InfyMetaFixed m;
    uint8_t* blob = 0;
    uint64_t blob_len = 0;
    if (infy_art_read_file(argv[1], &m, &blob, &blob_len) != 0) return 1;
    infy_art_dump_meta(&m, blob, blob_len, stdout);
    free(blob);
    return 0;
}
