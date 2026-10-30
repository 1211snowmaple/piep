import { expect,it } from "vitest";
import { imageReferenceInfo } from "./imageReference";

it("links malformed numeric references to the image without guessing the intended page",()=>{
  expect(imageReferenceInfo("[pixivimage:123-0]")).toEqual({url:"https://www.pixiv.net/artworks/123",issue:expect.stringContaining("0ページ目")});
  expect(imageReferenceInfo("[pixivimage:id=123]")).toEqual({url:"https://www.pixiv.net/artworks/123",issue:expect.stringContaining("指定方法")});
  expect(imageReferenceInfo("[pixivimage:123-2]")).toEqual({url:"https://www.pixiv.net/artworks/123",issue:undefined});
  expect(imageReferenceInfo("[uploadedimage:123]")).toEqual({});
  expect(imageReferenceInfo("[pixivimage:https://example.com]")).toEqual({});
});
