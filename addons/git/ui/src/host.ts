// The hub's helpers, through the Hub the addon was activated with.
import type { Hub } from "ue-dev-hub-addon-api";
import { hub } from "./api";

const ui = () => hub().ui;
const app = () => hub().app;

export const h: Hub["ui"]["h"] = (tag, attrs, ...children) => ui().h(tag, attrs, ...children);
export const icon: Hub["ui"]["icon"] = (...a) => ui().icon(...a);
export const toast: Hub["ui"]["toast"] = (...a) => ui().toast(...a);
export const modal: Hub["ui"]["modal"] = (build, onDismiss) => ui().modal(build, onDismiss);
export const confirmDialog: Hub["ui"]["confirmDialog"] = (...a) => ui().confirmDialog(...a);
export const errorText: Hub["ui"]["errorText"] = (e) => ui().errorText(e);
export const plural: Hub["ui"]["plural"] = (...a) => ui().plural(...a);
export const avatar: Hub["avatars"]["avatar"] = (...a) => hub().avatars.avatar(...a);
export const loadAvatars: Hub["avatars"]["loadAvatars"] = (emails) => hub().avatars.loadAvatars(emails);
export const busy = () => app().busy();
export const busyLabel = () => app().busyLabel();
export const rerender = () => app().rerender();
