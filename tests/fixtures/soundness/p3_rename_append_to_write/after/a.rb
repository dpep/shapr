class Sink
  def initialize = @out = []

  def write(s)
    @out << s
    s.size
  end
end

$stdout = Sink.new
