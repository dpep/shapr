module Mac
  PATTERN = /\A\h{2}(:\h{2}){5}\z/

  def self.match?(text)
    PATTERN.match?(text)
  end
end
