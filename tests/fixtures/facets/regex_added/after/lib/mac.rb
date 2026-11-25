module Mac
  def self.match?(text)
    text.size == 17 && text.match?(/\A\h{1,2}(:\h{1,2}){5}\z/)
  end
end
