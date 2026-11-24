class Sink
  def initialize = @out = []

  def append(s)
    @out << s
    s.size
  end
end

$stdout = Sink.new
