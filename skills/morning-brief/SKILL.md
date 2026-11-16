---
name: morning-brief
description: Xây bản tin sáng từ email và lịch, rồi đề xuất quyết định chờ duyệt.
triggers:
  - bản tin sáng
  - morning brief
  - hộp thư
  - quyết định chờ duyệt
  - lịch hôm nay
allowed-tools:
  - read_memory
  - draft_email
---

# Morning brief

Xây bản tin sáng từ ảnh chụp hộp thư chưa đọc và lịch, rồi đề xuất
quyết định cho những email cần người dùng quyết. Chỉ đọc, không gửi
gì, không đặt lịch gì khi chưa có duyệt.

## Khi dùng

- Người dùng hỏi bản tin sáng, lịch hôm nay, email nào cần quyết.
- Đầu ca làm việc, trước khi mở hộp thư.

## Cách làm

1. Lấy ảnh chụp email chưa đọc và sự kiện lịch, đưa vào
   `buildMorningBrief` trong `packages/core/src/morning-brief.ts`
   với `now` là thời điểm hiện tại và `windowDays` mặc định là 7.
2. Đọc bản tin: xung đột lịch (chồng lấn thật sự, tính bằng phút),
   hạn trong cửa sổ 7 ngày, email cần quyết. Sự kiện chỉ chạm nhau
   (cái này kết thúc đúng lúc cái kia bắt đầu) không phải xung đột.
3. Với mỗi email cần quyết, gọi `proposeDecisions` để có một đề
   xuất duy nhất cho mỗi email, id `proposal:<emailId>`, hành động
   mặc định là `reply`. Bản tóm tắt phải nêu rõ chủ đề, người gửi,
   và nói thẳng là người dùng phải duyệt trước.
4. Áp dụng một đề xuất chỉ qua `applyDecision`, đi qua cổng duyệt
   P0-2: không có quy tắc cho phép thì bị từ chối, có quy tắc mà
   chưa có duyệt thì chờ duyệt, công cụ tầng never thì luôn bị từ
   chối. Hành động `ignore` không cần cổng, chỉ đánh dấu đã xong
   tại chỗ.
5. Trình bày bản tin bằng `renderMorningBriefText`. Mục trống thì
   nói thẳng là trống, không bịa mục.

## Không làm

- Không gửi email, không đặt lịch, không lưu trữ email khi chưa có
  duyệt rõ ràng cho đúng công cụ và đúng idempotency key.
- Không đoán email nào cần quyết ngoài cờ `needsDecision` và danh
  sách marker trong code. Email thông tin thuần túy không phải
  mục cần quyết.
- Không tự ý đổi cửa sổ 7 ngày hay múi giờ so sánh (UTC) khi chưa
  có quyết.
