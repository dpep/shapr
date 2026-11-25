class Encoder
  def encode(params)
    return params.join("&")
  rescue TypeError
    ""
  end
end
