class Encoder
  def encode(params)
    buffer = params.map(&:to_s).join("&")
    return buffer.chop
  end
end
