# Nháp scaffold Sen — việc B-2 (chạy song song, tin 544), SD Developer

> as_of: đêm 30/10/2026, cập nhật 13:24 30/10/2026 · Phạm vi: CHỈ SOẠN NHÁP, không dựng repo, không commit, không merge, không tự verify.
> Cập nhật 13:24 30/10: B-1 đã duyệt tại nối ca 11:30 30/10 (tin 975: repo 7enlabs/sen-desktop, nhánh dev/scaffold). Toàn văn B-1 vẫn chưa có trong workspace; áp lên nhánh vẫn chờ đường vào box và file checklist của SD Leader.
> Lệnh: tin 544 — B-2 chạy SONG SONG từ đêm nay, không chờ B-1 duyệt xong. Duyệt B-1 lúc 11:30 30/10 sẽ điều chỉnh scaffold thay vì làm cổng chặn.
> Khung xương dưới đây kế thừa nháp C-1 (`sen-scaffold-draft-c1-2026-10-27.md`, 90 dòng) và ghi chú khả thi đã kiểm chứng; không bịa nội dung khung mới.

## 1. Khung B-S2 — tên đã biết, toàn văn chưa có trong workspace của ghế

- Tin 544 gọi khung là "khung B-S2 đã chốt tại rà 26/10". Tên gọi này mới xuất hiện ở tin 544; toàn văn khung vẫn chưa có trong workspace của ghế (file kế hoạch B-1 của SD Leader `sen-ke-hoach-scaffold-b1-nop-2026-10-30.md` không nằm trong workspace này).
- Hệ quả: nháp này giữ nguyên khung xương đã kiểm chứng từ C-1, chỉ áp thêm các ràng buộc của tin 544 ở mục 2. Duyệt B-1 đã xong 11:30 30/10; khi toàn văn B-S2/B-1 tới, đối chiếu và sửa nháp này — không giữ hai bản song song.
- Repo đích: đã duyệt 11:30 30/10 (tin 975): 7enlabs/sen-desktop, nhánh dev/scaffold. Áp lên nhánh vẫn chờ đường vào box.
- Nơi dựng: tin 544 giao làm trên box; box sd-developer chưa có đường vào (chờ chỉ đạo Leader) nên nháp này dựng dạng file trước, chuyển lên box khi có đường vào.

## 2. Ràng buộc tin 544 áp vào scaffold

- Không đưa cua-driver vào trước cổng Z Red 10/11.
- iOS hoãn; Android emulator trước.
- Đa nền tảng ngay từ kiến trúc (quyết định Mục 16): script trung lập hệ, một biên đường dẫn ở `packages/core`, không native addon ở tầng khung.
- Pin SDK của Sen giữ 1.3.0 theo quyết tin 457 (chuẩn 1.4.0 của đội là chuẩn công cụ, không đổi pin SDK).
- License lõi (tin 895, kiểm chứng 30/10): npm registry xác nhận @earendil-works/pi-coding-agent@1.3.0 và @earendil-works/pi-agent-core@1.3.0 đều license MIT (ghế tự kiểm chứng cả hai gói, khớp xác nhận tin 914 của SD Leader); MIT cho phép nhúng/phân phối kèm giữ thông báo bản quyền — đáp ứng điều kiện 1, scaffold sẽ mang thông báo MIT.
- Bọc lõi qua IPC (tin 895, theo xác nhận tin 914 của SD Leader, dẫn tin 323 mục 9): giữ Tauri invoke + IPC bridge — đáp ứng điều kiện 3 ở mức nháp; xác nhận đủ 3 điều kiện của ghế và áp lên nhánh dev/scaffold vẫn chờ (nhánh chưa có đường vào, SD Leader rà lại trước khi đóng B-2).
- Cổng thực thi duy nhất (tin 901 của SD Leader, ZC Leader nghiệm thu tin 910 làm đầu vào checklist; tin 931 của ZC Leader CHỐT RIÊNG áp ngay như các điểm tin 905, thay cho chờ chốt riêng nêu ở tin 916): mọi hành động thay đổi thế giới bên ngoài phải qua một cổng, theo thứ tự cố định: bảo vệ → kiểm tra khóa → kiểm tra hành động không đảo ngược và đồng ý → thực thi → kiểm tra sau → ghi lại; đi đường vòng hoặc thiếu chặng thì FAIL. Không áp: chế độ hỏi hẹp của Mika (chỉ chặn tiền), đổi chế độ giữa chừng hay vô hiệu lớp bảo vệ, phần riêng Android không chuyển sang desktop. Áp vào nháp này (lớp permission ở `packages/tools`); áp lên nhánh dev/scaffold vẫn chờ đường vào box và file checklist của SD Leader (tiến độ báo cùng các điểm 854-857, 888 theo tin 931).

## 3. Khung xương scaffold (kế thừa C-1, đã kiểm chứng)

- pnpm workspace: `apps/desktop` (host nhúng SDK, ESM-only) + `packages/core` (bọc `createAgentSession`, log sự kiện thô tách khỏi lớp suy ra) + `packages/providers` (ModelRuntime, khóa version Pi) + `packages/tools` (customTools, lớp permission qua hook `tool_call`) + `packages/prompts` + `packages/ui` + `evals/` + `docs/` và dotfiles (`.gitattributes` chốt LF, `.editorconfig` thống nhất thụt lề từ commit đầu).
- Luật bộ nhớ Sen áp vào khung (trình chốt, chưa vào Sổ): log thô là nguồn sự thật; suy ra mặc định HUỶ trừ khi qua cổng kiểm chứng; xóa phải đóng kín hậu duệ (AfterDeleteBench).

## 4. Nháp này CỐ TÌNH KHÔNG QUYẾT

1. Toàn văn khung B-S2 — xem mục 1.
2. Repo đích — đã duyệt 11:30 30/10 (tin 975): 7enlabs/sen-desktop, nhánh dev/scaffold; chỉ còn chờ đường vào box để áp lên nhánh.
3. Công nghệ vỏ desktop — điểm mở từ ghi chú khả thi; khung không phụ thuộc lựa chọn này nếu giữ các package độc lập với vỏ.
4. Thời điểm chuyển nháp lên box — chờ đường vào box.

## 5. Bước tiếp theo

- Tiến độ B-2 báo tại nối ca 03:30 30/10 (tin 544): đã bắt đầu đêm nay, nháp file này, box đang chờ.
- B-1 đã duyệt 11:30 30/10 (tin 975): điều chỉnh scaffold theo quyết duyệt, không làm lại từ đầu; đối chiếu toàn văn B-1 vẫn chờ vì toàn văn chưa có.
- Cổng Z Red 10/11: chỉ sau cổng này mới xét đưa cua-driver vào.
