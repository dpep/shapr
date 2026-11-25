class Encoder
  def encode(params)
    params.join("&")
  rescue TypeError
    ""
  end
end
