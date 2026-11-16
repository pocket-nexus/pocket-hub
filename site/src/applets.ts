// The applets the page shows: the ones the store lists at applets.pocket.nexus,
// with their icons as the applets repository keeps them (pocket-nexus/applets,
// <applet>/icon.png, 128 px; GPL-3.0-or-later like this repository). They
// are pictures of what the store offers; the page links the store itself, and
// a change of the catalog does not have to reach this list.
import discord from "./assets/applets/discord.png";
import github from "./assets/applets/github.png";
import hn from "./assets/applets/hn.png";
import homeassistant from "./assets/applets/homeassistant.png";
import maps from "./assets/applets/maps.png";
import podcasts from "./assets/applets/podcasts.png";
import spotify from "./assets/applets/spotify.png";
import weather from "./assets/applets/weather.png";
import x from "./assets/applets/x.png";
import youtube from "./assets/applets/youtube.png";

export interface Applet {
  id: string;
  title: string;
  icon: string;
}

export const APPLETS: Applet[] = [
  { id: "dev.pocket-nexus.weather", title: "Weather", icon: weather },
  { id: "dev.pocket-nexus.hn", title: "Hacker News", icon: hn },
  { id: "dev.pocket-nexus.maps", title: "Maps", icon: maps },
  { id: "dev.pocket-nexus.podcasts", title: "Podcasts", icon: podcasts },
  { id: "dev.pocket-nexus.github", title: "GitHub", icon: github },
  { id: "dev.pocket-nexus.homeassistant", title: "Home", icon: homeassistant },
  { id: "dev.pocket-nexus.youtube", title: "YouTube", icon: youtube },
  { id: "dev.pocket-nexus.spotify", title: "Spotify", icon: spotify },
  { id: "dev.pocket-nexus.discord", title: "Discord", icon: discord },
  { id: "dev.pocket-nexus.x", title: "X", icon: x },
];

export const STORE = "https://applets.pocket.nexus";
export const REPOSITORY_URL = "https://github.com/pocket-nexus/pocket-hub";
