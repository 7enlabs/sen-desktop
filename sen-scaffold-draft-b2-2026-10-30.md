# Nháp scaffold Sen — việc B-2 (chạy song song, tin 544), SD Developer

> as_of: đêm 30/10/2026, cập nhật 17:10 30/10/2026 · Phạm vi: CHỈ SOẠN NHÁP, không dựng repo, không commit, không merge, không tự verify.
> Cập nhật 13:24 30/10: B-1 đã duyệt tại nối ca 11:30 30/10 (tin 975: repo 7enlabs/sen-desktop, nhánh dev/scaffold). Toàn văn B-1 vẫn chưa có trong workspace; áp lên nhánh vẫn chờ đường vào box và file checklist của SD Leader.
> Cập nhật 17:10 30/10 (theo rà B-2 tin 1060 của SD Leader, trả tin 991): bổ sung đầy đủ các mục 854–857, 888 và A1–A4 (tin 962) vào nháp này ở mục 2b. Bản này vẫn là nháp local trên máy htch-runtime; áp lên nhánh dev/scaffold vẫn chờ đường vào nhánh (ghế không có đường vào), SD Leader rà lại sau khi bản cập nhật lên nhánh. Chưa đọc toàn văn phiếu rà tin 1059 (helper Telegram chỉ trả caption, không tải được tài liệu đính kèm) nên không xác nhận nội dung phiếu rà.
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

## 2b. Điểm áp dụng đã nghiệm thu bổ sung vào nháp (theo rà B-2 tin 1060)

Các điểm dưới đây trích nguyên văn tin nhóm của SD Leader (không bịa, không diễn giải thêm); ZC Leader đã nghiệm thu 854–857 làm đầu vào checklist B-2 (tin 868), nghiệm thu 888 (tin 896), và chốt áp ngay 854–857 + 888 vào scaffold B-2 (tin 905). Áp lên nhánh dev/scaffold vẫn chờ đường vào nhánh.

- 854 — statechart + kiểm chứng hình thức (tin 595/598): tiêu chí 3 kiểm tra cho phiên agent Sen — bị từ chối tại AwaitingApproval phải về Cancelled, không treo; Error phải đủ 2 đường (về Running và sang Failed); chỉ 3 trạng thái kết thúc Completed/Cancelled/Failed mới không có đường ra. Checklist rà B-2: critic-checklist-b2-statechart-2026-10-30.md.
- 855 — compaction vs memory (tin 629): tóm tắt compaction chỉ là khung nhìn dẫn xuất, phải dựng lại được từ log thô; cấm dùng làm căn cứ quyết L3/L4 hay hồ sơ bàn giao; đổi nhiệm vụ thì mở phiên mới thay vì compact tiếp. Checklist rà B-2: critic-checklist-b2-compaction-2026-10-30.md.
- 856 — TokenEconomizer (tin 682): tiêu chí turn kẹt cho phiên Sen — cùng lỗi lặp lại qua các lần gọi tool, hoặc turn dài quá 2,5 lần mức thường trong khi churn cùng một edit, thì redirect tối đa 1 lần, sau đó chỉ báo dừng; bằng chứng (chữ ký lỗi + độ dài turn) ghi vào log thô. Checklist rà B-2: critic-checklist-b2-token-economizer-2026-10-30.md.
- 857 — multi-agent-repo-protocol (tin 765): mô tả PR/bàn giao phải liệt kê claim theo đường dẫn; người rà đối chiếu git diff --stat, file ngoài claim mà không kèm dòng lý do thì FAIL, không duyệt tới khi claim và diff khớp nhau. Checklist rà B-2: critic-checklist-b2-multi-agent-repo-protocol-2026-10-30.md.
- 888 — ai-agent-handbook (tin 822): tiêu chí hợp đồng hành động cho IPC bridge của Sen — mỗi lần gọi tool phải mang id, năng lực + phiên bản, actor, mục đích + giai đoạn, phân loại tác dụng phụ/khả đảo ngược/rủi ro, idempotency key + timeout, yêu cầu phê duyệt/kiểm toán; và phân biệt rõ tool model thấy ≠ tool đã đăng ký ≠ tool được ủy quyền thực thi. Checklist rà B-2: critic-checklist-b2-ai-agent-handbook-2026-10-30.md. Critic PASS có điều kiện, verdict HỌC THIẾT KẾ; không áp pipeline ghi memory tự động, tầng gateway doanh nghiệp, hay identity/thanh toán của Personal Agent.
- A1–A4 — tin 962 (SD Leader, tin 917, critic PASS có điều kiện, verdict HỌC THIẾT KẾ): (A1) mọi lời gọi model kể cả phụ đều mang hợp đồng IPC và có dòng log thô, thiếu log thì FAIL; (A2) lời gọi model chỉ phục vụ hiển thị mặc định TẮT; (A3) cấm cổng vá runtime bên thứ ba, tối ưu dạng chặn phải có tín hiệu kiểm chứng dương và kiểm lại sau mỗi lần đổi SDK/bản ghim; (A4) báo cáo ngân sách token theo ca tách riêng cột lời gọi phụ. KHÔNG áp: không cài hook Bun preload, không vá runtime Pi, không lấy 16,4% làm định mức Sen. Điều kiện mở: chưa xác minh Pi SDK 1.3.0 có lời gọi phụ dạng này hay không, người rà đối chiếu dossier trước khi kết luận.

## 3. Khung xương scaffold (kế thừa C-1, đã kiểm chứng)

- pnpm workspace: `apps/desktop` (host nhúng SDK, ESM-only) + `packages/core` (bọc `createAgentSession`, log sự kiện thô tách khỏi lớp suy ra) + `packages/providers` (ModelRuntime, khóa version Pi) + `packages/tools` (customTools, lớp permission qua hook `tool_call`) + `packages/prompts` + `packages/ui` + `evals/` + `docs/` và dotfiles (`.gitattributes` chốt LF, `.editorconfig` thống nhất thụt lề từ commit đầu).
- Luật bộ nhớ Sen áp vào khung (trình chốt, chưa vào Sổ): log thô là nguồn sự thật; suy ra mặc định HUỶ trừ khi qua cổng kiểm chứng; xóa phải đóng kín hậu duệ (AfterDeleteBench).

## 4. Nháp này CỐ TÌNH KHÔNG QUYẾT

1. Toàn văn khung B-S2 — xem mục 1.
2. Repo đích — đã duyệt 11:30 30/10 (tin 975): 7enlabs/sen-desktop, nhánh dev/scaffold; chỉ còn chờ đường vào box để áp lên nhánh.
3. Công nghệ vỏ desktop — điểm mở từ ghi chú khả thi; khung không phụ thuộc lựa chọn này nếu giữ các package độc lập với vỏ.
4. Thời điểm chuyển nháp lên box — chờ đường vào box.

## 5. Bước tiếp theo

- 17:10 30/10: đã bổ sung 854–857, 888, A1–A4 vào nháp này theo tin 1060; bản cập nhật gửi lên nhóm làm bản dự phòng để Leader/SD Leader áp lên nhánh dev/scaffold (ghế vẫn không có đường vào nhánh, chưa áp trực tiếp được); chờ SD Leader rà lại trong ca theo tin 1060.
- Tiến độ B-2 báo tại nối ca 03:30 30/10 (tin 544): đã bắt đầu đêm nay, nháp file này, box đang chờ.
- B-1 đã duyệt 11:30 30/10 (tin 975): điều chỉnh scaffold theo quyết duyệt, không làm lại từ đầu; đối chiếu toàn văn B-1 vẫn chờ vì toàn văn chưa có.
- Cổng Z Red 10/11: chỉ sau cổng này mới xét đưa cua-driver vào.
