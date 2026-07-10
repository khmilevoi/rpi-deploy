FROM busybox:1.37
WORKDIR /www
COPY health /www/health
EXPOSE 8080
CMD ["httpd", "-f", "-p", "8080", "-h", "/www"]
