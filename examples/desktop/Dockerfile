# A Linux desktop you can drive from a browser: XFCE, GIMP and a text editor,
# served over noVNC on port 6080.
FROM mirror.gcr.io/library/ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      xfce4 xfce4-terminal mousepad gimp dbus-x11 \
      tigervnc-standalone-server tigervnc-tools novnc websockify \
      fonts-dejavu-core adwaita-icon-theme-full librsvg2-common libasound2t64 ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN useradd -m -s /bin/bash demo
COPY start.sh /start.sh
USER demo
WORKDIR /home/demo
EXPOSE 6080
CMD ["/start.sh"]
