FROM crpi-3cfwwtc3um8h6d3q.cn-hangzhou.personal.cr.aliyuncs.com/job-copilot/api-v2:preprod

USER root
COPY target/release/ponyllm /usr/local/bin/ponyllm
COPY web/dist /opt/ponyllm/web/dist
RUN chmod +x /usr/local/bin/ponyllm

EXPOSE 8080
ENTRYPOINT ["ponyllm"]
CMD ["serve", "--bind", "0.0.0.0:8080"]
