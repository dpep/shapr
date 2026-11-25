class Client
  def configure(req)
    if req[:timeout]
      client.connect_timeout = req[:timeout]
      client.receive_timeout = req[:timeout]
      client.send_timeout = req[:timeout]
    end
  end
end
