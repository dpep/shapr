class Client
  def configure(req)
    configure_timeout(req) if req[:timeout]
  end

  def configure_timeout(req)
    client.connect_timeout = req[:timeout]
    client.receive_timeout = req[:timeout]
    client.send_timeout = req[:timeout]
  end
end
